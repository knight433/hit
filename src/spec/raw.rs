//! Permissive serde structs over openapi.json. Everything defaults, unknown
//! fields are ignored, and schemas stay as raw `serde_json::Value` until the
//! normalization pass. This is deliberately ~20% of OpenAPI: enough for
//! browsing endpoints and building request bodies.

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawSpec {
    #[serde(default)]
    pub openapi: String,
    #[serde(default)]
    pub info: RawInfo,
    #[serde(default)]
    pub paths: IndexMap<String, RawPathItem>,
    /// Spec-level tag declarations (order defines display order).
    #[serde(default)]
    pub tags: Vec<RawTagDecl>,
    /// Spec-level default security requirement.
    #[serde(default)]
    pub security: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawInfo {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawTagDecl {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawPathItem {
    pub get: Option<RawOperation>,
    pub put: Option<RawOperation>,
    pub post: Option<RawOperation>,
    pub delete: Option<RawOperation>,
    pub options: Option<RawOperation>,
    pub head: Option<RawOperation>,
    pub patch: Option<RawOperation>,
    pub trace: Option<RawOperation>,
    /// Parameters shared by all operations on this path.
    #[serde(default)]
    pub parameters: Vec<Value>,
}

impl RawPathItem {
    pub fn operations(&self) -> impl Iterator<Item = (&'static str, &RawOperation)> {
        [
            ("GET", &self.get),
            ("PUT", &self.put),
            ("POST", &self.post),
            ("DELETE", &self.delete),
            ("OPTIONS", &self.options),
            ("HEAD", &self.head),
            ("PATCH", &self.patch),
            ("TRACE", &self.trace),
        ]
        .into_iter()
        .filter_map(|(m, op)| op.as_ref().map(|o| (m, o)))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawOperation {
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub operation_id: Option<String>,
    /// Kept as Values: a parameter may itself be a `$ref`.
    #[serde(default)]
    pub parameters: Vec<Value>,
    /// Kept as a Value: may be a `$ref`; content negotiated during build.
    #[serde(default)]
    pub request_body: Option<Value>,
    /// `None` = inherit spec-level security; `Some([])` = explicitly public.
    #[serde(default)]
    pub security: Option<Vec<Value>>,
    #[serde(default)]
    pub deprecated: bool,
    /// Status code -> response object (kept raw; may contain `$ref`s).
    #[serde(default)]
    pub responses: IndexMap<String, Value>,
}

/// A parameter object after any `$ref` indirection has been resolved.
#[derive(Debug, Clone, Deserialize)]
pub struct RawParameter {
    pub name: String,
    #[serde(rename = "in")]
    pub location: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub schema: Option<Value>,
}
