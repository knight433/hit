//! Framework-specific behavior. OpenAPI itself is framework-neutral; the
//! adapter isolates the conventions that aren't, so adding Django/Express
//! support later means a new impl here, not changes across the codebase.

use serde_json::Value;

use crate::config::Framework;

pub trait FrameworkAdapter: Send + Sync {
    fn name(&self) -> &'static str;

    /// Pretty-render a framework-specific error body into one line per
    /// problem, or `None` to fall back to generic JSON rendering.
    fn render_error_lines(&self, status: u16, body: &Value) -> Option<Vec<String>>;

    /// Component schema names that are framework plumbing, not domain models.
    fn validation_schemas(&self) -> &'static [&'static str];
}

pub fn adapter_for(framework: Framework) -> &'static dyn FrameworkAdapter {
    match framework {
        Framework::Fastapi => &FastApiAdapter,
    }
}

pub struct FastApiAdapter;

impl FrameworkAdapter for FastApiAdapter {
    fn name(&self) -> &'static str {
        "fastapi"
    }

    /// FastAPI 422: `{"detail": [{"loc": ["body", "email"], "msg": "...", "type": "..."}]}`
    /// becomes `body.email: <msg>` lines. A plain string `detail` (HTTPException)
    /// is rendered as-is.
    fn render_error_lines(&self, status: u16, body: &Value) -> Option<Vec<String>> {
        let detail = body.get("detail")?;
        if let Some(message) = detail.as_str() {
            return Some(vec![format!("{status}: {message}")]);
        }
        let items = detail.as_array()?;
        let lines: Vec<String> = items
            .iter()
            .filter_map(|item| {
                let msg = item.get("msg")?.as_str()?;
                let loc = item
                    .get("loc")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .map(|p| match p {
                                Value::String(s) => s.clone(),
                                other => other.to_string(),
                            })
                            .collect::<Vec<_>>()
                            .join(".")
                    })
                    .unwrap_or_default();
                Some(if loc.is_empty() {
                    msg.to_string()
                } else {
                    format!("{loc}: {msg}")
                })
            })
            .collect();
        if lines.is_empty() { None } else { Some(lines) }
    }

    fn validation_schemas(&self) -> &'static [&'static str] {
        &["HTTPValidationError", "ValidationError"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_422_detail_lines() {
        let body = json!({"detail": [
            {"loc": ["body", "email"], "msg": "field required", "type": "missing"},
            {"loc": ["query", "limit"], "msg": "must be positive", "type": "value_error"}
        ]});
        let lines = FastApiAdapter.render_error_lines(422, &body).unwrap();
        assert_eq!(lines[0], "body.email: field required");
        assert_eq!(lines[1], "query.limit: must be positive");
    }

    #[test]
    fn renders_string_detail() {
        let body = json!({"detail": "Not authenticated"});
        let lines = FastApiAdapter.render_error_lines(401, &body).unwrap();
        assert_eq!(lines, vec!["401: Not authenticated"]);
    }

    #[test]
    fn ignores_foreign_bodies() {
        assert!(
            FastApiAdapter
                .render_error_lines(500, &json!({"error": "boom"}))
                .is_none()
        );
    }
}
