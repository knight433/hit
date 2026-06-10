//! The normalization pass: raw OpenAPI 3.0/3.1 JSON-schema fragments become
//! `SchemaNode`s. All version differences are erased here — `nullable: true`
//! (3.0), `type: ["T", "null"]` (3.1), and `anyOf: [T, {type: "null"}]`
//! (FastAPI's `Optional[T]` encoding) all collapse to the same shape.
//!
//! Malformed or unsupported schemas degrade to `SchemaNode::Any` (raw JSON
//! editing in the TUI) rather than failing the endpoint or the project.

use serde_json::Value;

use crate::model::{Field, OneOfVariant, SchemaNode};
use crate::spec::resolve;

/// Hard recursion cap (beyond ref-cycle detection) for pathological nesting.
const MAX_DEPTH: usize = 48;

/// A normalized schema plus the field-level attributes that live alongside
/// the type in JSON Schema but belong to `Field` in our model.
#[derive(Debug, Clone)]
pub struct Normalized {
    pub node: SchemaNode,
    pub nullable: bool,
    pub default: Option<Value>,
    pub description: Option<String>,
    pub title: Option<String>,
    pub read_only: bool,
}

impl Normalized {
    pub(crate) fn any() -> Self {
        Self {
            node: SchemaNode::Any,
            nullable: false,
            default: None,
            description: None,
            title: None,
            read_only: false,
        }
    }
}

/// Entry point: normalize `schema` against the root spec document.
pub fn normalize(doc: &Value, schema: &Value) -> Normalized {
    let mut visited = Vec::new();
    normalize_inner(doc, schema, &mut visited, 0)
}

fn normalize_inner(
    doc: &Value,
    schema: &Value,
    visited: &mut Vec<String>,
    depth: usize,
) -> Normalized {
    if depth > MAX_DEPTH {
        return Normalized::any();
    }
    // Boolean schemas (3.1): `true` = anything, `false` = nothing sensible.
    let Some(obj) = schema.as_object() else {
        return Normalized::any();
    };

    // Attributes that may sit beside $ref/allOf/anyOf and must survive merging.
    let local_default = obj.get("default").cloned();
    let local_description = string_of(obj.get("description"));
    let local_title = string_of(obj.get("title"));
    let local_read_only = obj
        .get("readOnly")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let local_nullable = obj
        .get("nullable")
        .and_then(Value::as_bool)
        .unwrap_or(false); // 3.0

    let overlay = |mut inner: Normalized| -> Normalized {
        inner.default = local_default.clone().or(inner.default);
        inner.description = local_description.clone().or(inner.description);
        inner.title = local_title.clone().or(inner.title);
        inner.read_only = inner.read_only || local_read_only;
        inner.nullable = inner.nullable || local_nullable;
        inner
    };

    // --- $ref ---------------------------------------------------------
    if let Some(reference) = obj.get("$ref").and_then(Value::as_str) {
        if visited.iter().any(|r| r == reference) {
            // Recursive model: cut the cycle, degrade this arm to Any.
            tracing::debug!(reference, "recursive $ref; emitting Any");
            return overlay(Normalized::any());
        }
        let (target, _) = resolve::deref(doc, schema);
        visited.push(reference.to_string());
        let inner = normalize_inner(doc, target, visited, depth + 1);
        visited.pop();
        return overlay(inner);
    }

    // --- allOf: merge -------------------------------------------------
    if let Some(parts) = obj.get("allOf").and_then(Value::as_array) {
        return overlay(merge_all_of(doc, parts, visited, depth));
    }

    // --- anyOf / oneOf: strip null variants, collapse or branch --------
    for key in ["anyOf", "oneOf"] {
        if let Some(variants) = obj.get(key).and_then(Value::as_array) {
            return overlay(normalize_variants(doc, variants, visited, depth));
        }
    }

    // --- const / enum ---------------------------------------------------
    if let Some(value) = obj.get("const") {
        return overlay(Normalized {
            node: SchemaNode::Const {
                value: value.clone(),
            },
            ..Normalized::any()
        });
    }

    // --- type (string in 3.0, possibly an array in 3.1) ------------------
    let (type_name, type_nullable) = extract_type(obj.get("type"));
    let mut result = match type_name.as_deref() {
        Some("object") => normalize_object(doc, obj, visited, depth),
        Some("array") => normalize_array(doc, obj, visited, depth),
        Some("string") => Normalized {
            node: SchemaNode::String {
                enum_values: enum_strings(obj.get("enum")),
                format: string_of(obj.get("format")),
            },
            ..Normalized::any()
        },
        Some("integer") => Normalized {
            node: SchemaNode::Integer {
                minimum: obj.get("minimum").and_then(Value::as_i64),
                maximum: obj.get("maximum").and_then(Value::as_i64),
                enum_values: enum_ints(obj.get("enum")),
            },
            ..Normalized::any()
        },
        Some("number") => Normalized {
            node: SchemaNode::Number {
                minimum: obj.get("minimum").and_then(Value::as_f64),
                maximum: obj.get("maximum").and_then(Value::as_f64),
            },
            ..Normalized::any()
        },
        Some("boolean") => Normalized {
            node: SchemaNode::Boolean,
            ..Normalized::any()
        },
        Some("null") => Normalized {
            node: SchemaNode::Any,
            nullable: true,
            ..Normalized::any()
        },
        Some(other) => {
            tracing::debug!(r#type = other, "unknown schema type; emitting Any");
            Normalized::any()
        }
        // No explicit type: infer from structure.
        None => {
            if obj.contains_key("properties") || obj.contains_key("additionalProperties") {
                normalize_object(doc, obj, visited, depth)
            } else if obj.contains_key("items") {
                normalize_array(doc, obj, visited, depth)
            } else if let Some(values) = enum_strings(obj.get("enum")) {
                Normalized {
                    node: SchemaNode::String {
                        enum_values: Some(values),
                        format: None,
                    },
                    ..Normalized::any()
                }
            } else {
                Normalized::any()
            }
        }
    };

    // Single-value enums behave like const (Pydantic `Literal["x"]`).
    if let SchemaNode::String {
        enum_values: Some(values),
        ..
    } = &result.node
        && values.len() == 1
    {
        result.node = SchemaNode::Const {
            value: Value::String(values[0].clone()),
        };
    }

    result.nullable = result.nullable || type_nullable;
    overlay(result)
}

/// `type` may be a string ("string") or, in 3.1, an array (["string", "null"]).
fn extract_type(type_value: Option<&Value>) -> (Option<String>, bool) {
    match type_value {
        Some(Value::String(s)) => (Some(s.clone()), false),
        Some(Value::Array(items)) => {
            let mut nullable = false;
            let mut name = None;
            for item in items {
                match item.as_str() {
                    Some("null") => nullable = true,
                    Some(other) if name.is_none() => name = Some(other.to_string()),
                    _ => {}
                }
            }
            (name, nullable)
        }
        _ => (None, false),
    }
}

fn normalize_object(
    doc: &Value,
    obj: &serde_json::Map<String, Value>,
    visited: &mut Vec<String>,
    depth: usize,
) -> Normalized {
    let required: Vec<&str> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut fields = Vec::new();
    if let Some(props) = obj.get("properties").and_then(Value::as_object) {
        for (name, prop_schema) in props {
            let normalized = normalize_inner(doc, prop_schema, visited, depth + 1);
            fields.push(Field {
                name: name.clone(),
                required: required.contains(&name.as_str()),
                nullable: normalized.nullable,
                default: normalized.default,
                description: normalized.description,
                read_only: normalized.read_only,
                schema: normalized.node,
            });
        }
    }

    let additional = match obj.get("additionalProperties") {
        Some(Value::Bool(true)) => Some(Box::new(SchemaNode::Any)),
        Some(Value::Object(_)) => {
            let normalized = normalize_inner(doc, &obj["additionalProperties"], visited, depth + 1);
            Some(Box::new(normalized.node))
        }
        _ => None,
    };

    Normalized {
        node: SchemaNode::Object { fields, additional },
        ..Normalized::any()
    }
}

fn normalize_array(
    doc: &Value,
    obj: &serde_json::Map<String, Value>,
    visited: &mut Vec<String>,
    depth: usize,
) -> Normalized {
    let item = obj
        .get("items")
        .map(|items| normalize_inner(doc, items, visited, depth + 1).node)
        .unwrap_or(SchemaNode::Any);
    Normalized {
        node: SchemaNode::Array {
            item: Box::new(item),
            min_items: obj.get("minItems").and_then(Value::as_u64),
            max_items: obj.get("maxItems").and_then(Value::as_u64),
        },
        ..Normalized::any()
    }
}

/// allOf merge. The common FastAPI 3.0-era pattern is
/// `allOf: [{$ref: Model}]` with default/description as siblings; the general
/// case merges object fields left-to-right (later parts override by name).
fn merge_all_of(
    doc: &Value,
    parts: &[Value],
    visited: &mut Vec<String>,
    depth: usize,
) -> Normalized {
    let normalized: Vec<Normalized> = parts
        .iter()
        .map(|p| normalize_inner(doc, p, visited, depth + 1))
        .collect();

    if normalized.is_empty() {
        return Normalized::any();
    }
    if normalized.len() == 1 {
        return normalized.into_iter().next().unwrap();
    }

    // If every part is an object, merge their fields.
    let all_objects = normalized
        .iter()
        .all(|n| matches!(n.node, SchemaNode::Object { .. }));
    if all_objects {
        let mut merged_fields: Vec<Field> = Vec::new();
        let mut merged_additional = None;
        let mut nullable = false;
        for part in &normalized {
            nullable = nullable || part.nullable;
            if let SchemaNode::Object { fields, additional } = &part.node {
                for field in fields {
                    if let Some(existing) = merged_fields.iter_mut().find(|f| f.name == field.name)
                    {
                        *existing = field.clone();
                    } else {
                        merged_fields.push(field.clone());
                    }
                }
                if additional.is_some() {
                    merged_additional = additional.clone();
                }
            }
        }
        return Normalized {
            node: SchemaNode::Object {
                fields: merged_fields,
                additional: merged_additional,
            },
            nullable,
            ..Normalized::any()
        };
    }

    // Heterogeneous allOf: take the first concrete part.
    normalized
        .into_iter()
        .find(|n| n.node != SchemaNode::Any)
        .unwrap_or_else(Normalized::any)
}

/// anyOf/oneOf handling: null variants set `nullable`; a single remaining
/// variant collapses (the FastAPI `Optional[T]` pattern); several remaining
/// variants become `OneOf`.
fn normalize_variants(
    doc: &Value,
    variants: &[Value],
    visited: &mut Vec<String>,
    depth: usize,
) -> Normalized {
    let mut nullable = false;
    let mut concrete = Vec::new();
    for variant in variants {
        if is_null_schema(variant) {
            nullable = true;
            continue;
        }
        concrete.push(normalize_inner(doc, variant, visited, depth + 1));
    }

    match concrete.len() {
        0 => Normalized {
            nullable: true,
            ..Normalized::any()
        },
        1 => {
            let mut single = concrete.into_iter().next().unwrap();
            single.nullable = single.nullable || nullable;
            single
        }
        _ => {
            let variants = concrete
                .into_iter()
                .enumerate()
                .map(|(i, n)| OneOfVariant {
                    label: n
                        .title
                        .clone()
                        .unwrap_or_else(|| format!("{} #{}", n.node.kind_label(), i + 1)),
                    node: n.node,
                })
                .collect();
            Normalized {
                node: SchemaNode::OneOf { variants },
                nullable,
                ..Normalized::any()
            }
        }
    }
}

fn is_null_schema(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("null")
}

fn string_of(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_string)
}

fn enum_strings(value: Option<&Value>) -> Option<Vec<String>> {
    let values: Vec<String> = value?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

fn enum_ints(value: Option<&Value>) -> Option<Vec<i64>> {
    let values: Vec<i64> = value?
        .as_array()?
        .iter()
        .filter_map(Value::as_i64)
        .collect();
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn norm(schema: Value) -> Normalized {
        normalize(&json!({}), &schema)
    }

    #[test]
    fn openapi30_nullable_flag() {
        let n = norm(json!({"type": "string", "nullable": true}));
        assert!(n.nullable);
        assert!(matches!(n.node, SchemaNode::String { .. }));
    }

    #[test]
    fn openapi31_type_array_null() {
        let n = norm(json!({"type": ["string", "null"]}));
        assert!(n.nullable);
        assert!(matches!(n.node, SchemaNode::String { .. }));
    }

    #[test]
    fn fastapi_optional_anyof_null() {
        // Optional[str] on FastAPI/Pydantic v2: anyOf [string, null]
        let n = norm(json!({"anyOf": [{"type": "string"}, {"type": "null"}], "title": "Name"}));
        assert!(n.nullable);
        assert!(matches!(n.node, SchemaNode::String { .. }));
    }

    #[test]
    fn optional_union_keeps_oneof_and_nullable() {
        let n = norm(json!({
            "anyOf": [
                {"type": "string", "title": "Str"},
                {"type": "integer", "title": "Int"},
                {"type": "null"}
            ]
        }));
        assert!(n.nullable);
        let SchemaNode::OneOf { variants } = &n.node else {
            panic!("expected OneOf, got {:?}", n.node)
        };
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].label, "Str");
    }

    #[test]
    fn allof_single_ref_with_sibling_default() {
        // FastAPI 3.0-era: field with default referencing an enum model.
        let doc = json!({"components": {"schemas": {
            "Color": {"type": "string", "enum": ["red", "blue"]}
        }}});
        let schema = json!({"allOf": [{"$ref": "#/components/schemas/Color"}], "default": "red"});
        let n = normalize(&doc, &schema);
        assert_eq!(n.default, Some(json!("red")));
        assert!(matches!(&n.node, SchemaNode::String { enum_values: Some(v), .. } if v.len() == 2));
    }

    #[test]
    fn ref_resolution_and_required_orthogonal_to_nullable() {
        let doc = json!({"components": {"schemas": {
            "User": {
                "type": "object",
                "required": ["name", "nickname"],
                "properties": {
                    "name": {"type": "string"},
                    "nickname": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                    "bio": {"anyOf": [{"type": "string"}, {"type": "null"}], "default": null},
                    "level": {"type": "integer", "default": 3}
                }
            }
        }}});
        let n = normalize(&doc, &json!({"$ref": "#/components/schemas/User"}));
        let SchemaNode::Object { fields, .. } = &n.node else {
            panic!("expected object")
        };
        let get = |name: &str| fields.iter().find(|f| f.name == name).unwrap();
        // str -> required, not nullable
        assert!(get("name").required && !get("name").nullable);
        // Optional[str] (no default) -> required AND nullable
        assert!(get("nickname").required && get("nickname").nullable);
        // Optional[str] = None -> optional + nullable
        assert!(!get("bio").required && get("bio").nullable);
        // int = 3 -> optional, not nullable, default kept
        let level = get("level");
        assert!(!level.required && !level.nullable);
        assert_eq!(level.default, Some(json!(3)));
    }

    #[test]
    fn recursive_ref_degrades_to_any() {
        let doc = json!({"components": {"schemas": {
            "Node": {
                "type": "object",
                "properties": {
                    "value": {"type": "string"},
                    "child": {"anyOf": [{"$ref": "#/components/schemas/Node"}, {"type": "null"}]}
                }
            }
        }}});
        let n = normalize(&doc, &json!({"$ref": "#/components/schemas/Node"}));
        let SchemaNode::Object { fields, .. } = &n.node else {
            panic!("expected object")
        };
        let child = fields.iter().find(|f| f.name == "child").unwrap();
        assert_eq!(child.schema, SchemaNode::Any);
        assert!(child.nullable);
    }

    #[test]
    fn literal_single_enum_becomes_const() {
        let n = norm(json!({"type": "string", "enum": ["fixed"]}));
        assert_eq!(
            n.node,
            SchemaNode::Const {
                value: json!("fixed")
            }
        );
    }

    #[test]
    fn malformed_schema_degrades_to_any() {
        assert_eq!(norm(json!(true)).node, SchemaNode::Any);
        assert_eq!(norm(json!({"type": 42})).node, SchemaNode::Any);
        assert_eq!(norm(json!({})).node, SchemaNode::Any);
    }

    #[test]
    fn dict_str_model_open_map() {
        let n = norm(json!({
            "type": "object",
            "additionalProperties": {"type": "integer"}
        }));
        let SchemaNode::Object { fields, additional } = &n.node else {
            panic!("expected object")
        };
        assert!(fields.is_empty());
        assert!(matches!(
            additional.as_deref(),
            Some(SchemaNode::Integer { .. })
        ));
    }
}
