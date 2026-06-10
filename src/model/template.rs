//! RequestTemplate generation: turns an `Endpoint` into a fill-in-the-blanks
//! request description consumed by `hit template`, the MCP
//! `get_request_template` tool, and the TUI form seed.

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::{Endpoint, Field, ParamLocation, SchemaNode};

/// How deep example bodies auto-expand before degrading to `{}`/`[]`.
const MAX_EXAMPLE_DEPTH: usize = 6;

#[derive(Debug, Clone, Serialize)]
pub struct RequestTemplate {
    pub endpoint_id: String,
    pub method: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub path_params: Vec<TemplateField>,
    pub query_params: Vec<TemplateField>,
    pub header_params: Vec<TemplateField>,
    /// Example JSON body: defaults filled in, `<type:format>` placeholders
    /// elsewhere. Optional fields are present but listed in `optional_paths`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    /// The normalized body schema, for consumers that want the full shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_schema: Option<SchemaNode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_content_type: Option<String>,
    /// Dotted body paths that may be omitted entirely (e.g. "address.line2").
    pub optional_paths: Vec<String>,
    /// Dotted body paths that accept JSON null.
    pub nullable_paths: Vec<String>,
    pub auth_required: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateField {
    pub name: String,
    pub required: bool,
    pub nullable: bool,
    /// Placeholder or default value.
    pub value: Value,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

pub fn build_template(endpoint: &Endpoint) -> RequestTemplate {
    let mut optional_paths = Vec::new();
    let mut nullable_paths = Vec::new();

    let body = endpoint
        .body
        .as_ref()
        .map(|b| example_value(&b.schema, "", &mut optional_paths, &mut nullable_paths, 0));

    let param_fields = |location: ParamLocation| -> Vec<TemplateField> {
        endpoint
            .params_in(location)
            .map(|p| TemplateField {
                name: p.name.clone(),
                required: p.required,
                nullable: p.nullable,
                value: p.default.clone().unwrap_or_else(|| placeholder(&p.schema)),
                kind: p.schema.kind_label(),
                description: p.description.clone(),
            })
            .collect()
    };

    RequestTemplate {
        endpoint_id: endpoint.id.clone(),
        method: endpoint.method.clone(),
        path: endpoint.path.clone(),
        summary: endpoint.summary.clone(),
        path_params: param_fields(ParamLocation::Path),
        query_params: param_fields(ParamLocation::Query),
        header_params: param_fields(ParamLocation::Header),
        body,
        body_schema: endpoint.body.as_ref().map(|b| b.schema.clone()),
        body_content_type: endpoint.body.as_ref().map(|b| b.content_type.clone()),
        optional_paths,
        nullable_paths,
        auth_required: endpoint.auth_required,
    }
}

/// Build an example value for a schema node, recording optional/nullable
/// paths as we descend. `path` is the dotted location of this node's parent.
fn example_value(
    node: &SchemaNode,
    path: &str,
    optional: &mut Vec<String>,
    nullable: &mut Vec<String>,
    depth: usize,
) -> Value {
    if depth > MAX_EXAMPLE_DEPTH {
        return degraded(node);
    }
    match node {
        SchemaNode::Object { fields, additional } => {
            let mut map = Map::new();
            for field in fields {
                if field.read_only {
                    continue;
                }
                let child_path = join_path(path, &field.name);
                if !field.required {
                    optional.push(child_path.clone());
                }
                if field.nullable {
                    nullable.push(child_path.clone());
                }
                let value = field_example(field, &child_path, optional, nullable, depth);
                map.insert(field.name.clone(), value);
            }
            if fields.is_empty() && additional.is_some() {
                // Open map: show one illustrative key.
                if let Some(extra) = additional {
                    map.insert(
                        "<key>".to_string(),
                        example_value(extra, path, optional, nullable, depth + 1),
                    );
                }
            }
            Value::Object(map)
        }
        SchemaNode::Array { item, .. } => {
            let item_path = format!("{path}[]");
            json!([example_value(
                item,
                &item_path,
                optional,
                nullable,
                depth + 1
            )])
        }
        SchemaNode::OneOf { variants } => variants
            .first()
            .map(|v| example_value(&v.node, path, optional, nullable, depth + 1))
            .unwrap_or(Value::Null),
        _ => placeholder(node),
    }
}

fn field_example(
    field: &Field,
    child_path: &str,
    optional: &mut Vec<String>,
    nullable: &mut Vec<String>,
    depth: usize,
) -> Value {
    if let Some(default) = &field.default {
        return default.clone();
    }
    example_value(&field.schema, child_path, optional, nullable, depth + 1)
}

/// Leaf placeholder: enum/const pick a real value; scalars get a
/// `<type:format>` marker the caller is expected to replace.
fn placeholder(node: &SchemaNode) -> Value {
    match node {
        SchemaNode::String {
            enum_values: Some(values),
            ..
        } => values
            .first()
            .map(|v| Value::String(v.clone()))
            .unwrap_or(Value::Null),
        SchemaNode::String {
            format: Some(f), ..
        } => Value::String(format!("<string:{f}>")),
        SchemaNode::String { .. } => Value::String("<string>".to_string()),
        SchemaNode::Integer {
            enum_values: Some(values),
            ..
        } => values.first().map(|v| json!(v)).unwrap_or(Value::Null),
        SchemaNode::Integer { .. } => Value::String("<integer>".to_string()),
        SchemaNode::Number { .. } => Value::String("<number>".to_string()),
        SchemaNode::Boolean => Value::Bool(false),
        SchemaNode::Const { value } => value.clone(),
        SchemaNode::Any => json!({}),
        SchemaNode::Object { .. } | SchemaNode::Array { .. } | SchemaNode::OneOf { .. } => {
            degraded(node)
        }
    }
}

fn degraded(node: &SchemaNode) -> Value {
    match node {
        SchemaNode::Array { .. } => json!([]),
        _ => json!({}),
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}.{name}")
    }
}
