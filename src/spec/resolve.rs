//! Local `$ref` resolution against the spec document.

use serde_json::Value;

/// Resolve a `#/components/...`-style local reference against the root document.
/// Returns `None` for external refs, malformed pointers, or missing targets.
pub fn resolve_ref<'a>(doc: &'a Value, reference: &str) -> Option<&'a Value> {
    let pointer = reference.strip_prefix('#')?;
    // JSON pointer unescaping (~1 -> /, ~0 -> ~) is handled by Value::pointer.
    doc.pointer(pointer)
}

/// If `node` is a `{"$ref": "..."}` object, follow it (transitively, with a
/// hop limit guarding against ref-to-ref cycles). Returns the target and the
/// final ref string, or the node itself when it isn't a ref.
pub fn deref<'a>(doc: &'a Value, node: &'a Value) -> (&'a Value, Option<String>) {
    let mut current = node;
    let mut last_ref = None;
    for _ in 0..16 {
        let Some(reference) = current.get("$ref").and_then(Value::as_str) else {
            return (current, last_ref);
        };
        match resolve_ref(doc, reference) {
            Some(target) => {
                last_ref = Some(reference.to_string());
                current = target;
            }
            None => {
                tracing::warn!(reference, "unresolvable $ref; treating as empty schema");
                return (&Value::Null, last_ref);
            }
        }
    }
    tracing::warn!("$ref chain exceeded 16 hops; treating as empty schema");
    (&Value::Null, last_ref)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_component_schema() {
        let doc = json!({"components": {"schemas": {"User": {"type": "object"}}}});
        let node = json!({"$ref": "#/components/schemas/User"});
        let (resolved, reference) = deref(&doc, &node);
        assert_eq!(resolved, &json!({"type": "object"}));
        assert_eq!(reference.as_deref(), Some("#/components/schemas/User"));
    }

    #[test]
    fn missing_ref_degrades_to_null() {
        let doc = json!({});
        let node = json!({"$ref": "#/components/schemas/Nope"});
        let (resolved, _) = deref(&doc, &node);
        assert!(resolved.is_null());
    }

    #[test]
    fn non_ref_passes_through() {
        let doc = json!({});
        let node = json!({"type": "string"});
        let (resolved, reference) = deref(&doc, &node);
        assert_eq!(resolved, &node);
        assert!(reference.is_none());
    }
}
