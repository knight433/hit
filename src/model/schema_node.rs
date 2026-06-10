//! The normalized schema model shared by the TUI form, template generation,
//! and MCP output. Nothing downstream of `spec::normalize` sees raw OpenAPI.

use serde::Serialize;
use serde_json::Value;

/// A normalized JSON-schema shape. OpenAPI 3.0/3.1 differences (nullable,
/// type arrays, anyOf-null) are erased before this type is constructed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchemaNode {
    Object {
        fields: Vec<Field>,
        /// `additionalProperties` schema, when the object is an open map.
        #[serde(skip_serializing_if = "Option::is_none")]
        additional: Option<Box<SchemaNode>>,
    },
    Array {
        item: Box<SchemaNode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        min_items: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_items: Option<u64>,
    },
    String {
        #[serde(skip_serializing_if = "Option::is_none")]
        enum_values: Option<Vec<String>>,
        /// OpenAPI format hint: uuid, date-time, email, binary, ...
        #[serde(skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    Integer {
        #[serde(skip_serializing_if = "Option::is_none")]
        minimum: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        maximum: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        enum_values: Option<Vec<i64>>,
    },
    Number {
        #[serde(skip_serializing_if = "Option::is_none")]
        minimum: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        maximum: Option<f64>,
    },
    Boolean,
    /// Multiple non-null alternatives (anyOf/oneOf). The label comes from the
    /// variant's `title`, discriminator value, or type name.
    OneOf {
        variants: Vec<OneOfVariant>,
    },
    /// `const` or a single-value enum (Pydantic `Literal`).
    Const {
        value: Value,
    },
    /// Empty schema, unresolvable ref, recursion cutoff, or anything we choose
    /// not to model. Rendered as a raw-JSON editor in the TUI.
    Any,
}

impl SchemaNode {
    /// Short human label for display ("object", "string(uuid)", "enum", ...).
    pub fn kind_label(&self) -> String {
        match self {
            SchemaNode::Object { .. } => "object".into(),
            SchemaNode::Array { .. } => "array".into(),
            SchemaNode::String {
                enum_values: Some(_),
                ..
            } => "enum".into(),
            SchemaNode::String {
                format: Some(f), ..
            } => format!("string({f})"),
            SchemaNode::String { .. } => "string".into(),
            SchemaNode::Integer {
                enum_values: Some(_),
                ..
            } => "enum(int)".into(),
            SchemaNode::Integer { .. } => "integer".into(),
            SchemaNode::Number { .. } => "number".into(),
            SchemaNode::Boolean => "boolean".into(),
            SchemaNode::OneOf { .. } => "oneOf".into(),
            SchemaNode::Const { .. } => "const".into(),
            SchemaNode::Any => "any".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OneOfVariant {
    pub label: String,
    pub node: SchemaNode,
}

/// A named member of an object schema, or a request parameter.
///
/// `required` and `nullable` are orthogonal — this distinction drives the
/// TUI's Shift+X behavior and template `optional_paths`/`nullable_paths`:
/// - `Optional[str] = None`  -> optional + nullable (may omit, may send null)
/// - `Optional[str]`         -> required + nullable (must send, null allowed)
/// - `str = "x"`             -> optional, not nullable (may omit only)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Field {
    pub name: String,
    pub schema: SchemaNode,
    pub required: bool,
    pub nullable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `readOnly` fields are response-only; hidden from request forms.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
}
