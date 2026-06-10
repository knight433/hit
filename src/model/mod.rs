//! Core domain model: projects, parsed API specs, endpoints, templates.

mod schema_node;
mod template;

pub use schema_node::{Field, OneOfVariant, SchemaNode};
pub use template::{RequestTemplate, TemplateField, build_template};

use serde::Serialize;

/// A fully parsed and normalized API spec for one project.
#[derive(Debug, Clone, Serialize)]
pub struct ApiSpec {
    pub title: String,
    pub version: String,
    /// The raw `openapi` version string, e.g. "3.1.0".
    pub openapi_version: String,
    /// Spec tag order is preserved; endpoints with no tag land in "untagged".
    pub tags: Vec<TagGroup>,
    pub endpoints: Vec<Endpoint>,
}

impl ApiSpec {
    /// Look up an endpoint by operation_id or "METHOD /path" (case-insensitive
    /// method). On failure, returns close-ish suggestions.
    pub fn find_endpoint(&self, key: &str) -> Result<&Endpoint, crate::error::SpecError> {
        let normalized = key.trim();
        if let Some(ep) = self.endpoints.iter().find(|e| e.id == normalized) {
            return Ok(ep);
        }
        if let Some((method, path)) = normalized.split_once(' ') {
            let method = method.to_ascii_uppercase();
            if let Some(ep) = self
                .endpoints
                .iter()
                .find(|e| e.method == method && e.path == path.trim())
            {
                return Ok(ep);
            }
        }
        let needle = normalized.to_ascii_lowercase();
        let mut suggestions: Vec<String> = self
            .endpoints
            .iter()
            .filter(|e| {
                e.id.to_ascii_lowercase().contains(&needle)
                    || e.path.to_ascii_lowercase().contains(&needle)
            })
            .map(|e| e.id.clone())
            .collect();
        suggestions.truncate(5);
        Err(crate::error::SpecError::UnknownEndpoint {
            name: normalized.to_string(),
            suggestions,
        })
    }

    pub fn tag<'a>(&'a self, name: &str) -> Result<&'a TagGroup, crate::error::SpecError> {
        self.tags.iter().find(|t| t.name == name).ok_or_else(|| {
            crate::error::SpecError::UnknownTag {
                name: name.to_string(),
                available: self.tags.iter().map(|t| t.name.clone()).collect(),
            }
        })
    }

    pub fn endpoints_for_tag<'a>(&'a self, tag: &'a TagGroup) -> Vec<&'a Endpoint> {
        tag.endpoint_ids
            .iter()
            .filter_map(|id| self.endpoints.iter().find(|e| &e.id == id))
            .collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TagGroup {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub endpoint_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Endpoint {
    /// operation_id when present, otherwise "METHOD /path".
    pub id: String,
    /// Upper-case HTTP method.
    pub method: String,
    /// Path template with `{param}` placeholders, e.g. "/users/{user_id}".
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub deprecated: bool,
    pub params: Vec<Param>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<BodySpec>,
    /// Whether the operation declares an OpenAPI security requirement.
    pub auth_required: bool,
}

impl Endpoint {
    pub fn params_in(&self, location: ParamLocation) -> impl Iterator<Item = &Param> {
        self.params.iter().filter(move |p| p.location == location)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamLocation {
    Path,
    Query,
    Header,
}

#[derive(Debug, Clone, Serialize)]
pub struct Param {
    pub name: String,
    pub location: ParamLocation,
    pub required: bool,
    pub nullable: bool,
    pub schema: SchemaNode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BodySpec {
    /// The chosen request content type (application/json preferred).
    pub content_type: String,
    pub schema: SchemaNode,
    pub required: bool,
}
