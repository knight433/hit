//! Spec build + normalization + template generation against committed
//! FastAPI fixture specs (3.1 and 3.0-era output).

use hitpoint::model::{ParamLocation, SchemaNode, build_template};
use hitpoint::spec::build;
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let raw = std::fs::read_to_string(format!("tests/fixtures/{name}")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn builds_31_spec_with_tags_in_declared_order() {
    let spec = build(&fixture("fastapi_31.json")).unwrap();
    assert_eq!(spec.title, "Demo Shop API");
    assert_eq!(spec.openapi_version, "3.1.0");

    let tag_names: Vec<&str> = spec.tags.iter().map(|t| t.name.as_str()).collect();
    // Declared order first, then the untagged bucket (from /health).
    assert_eq!(tag_names, vec!["users", "items", "auth", "untagged"]);

    let users = spec.tag("users").unwrap();
    assert_eq!(users.endpoint_ids.len(), 4);
    assert_eq!(users.description.as_deref(), Some("User management"));
}

#[test]
fn endpoint_lookup_by_id_and_method_path() {
    let spec = build(&fixture("fastapi_31.json")).unwrap();
    assert!(spec.find_endpoint("create_user_users__post").is_ok());
    assert!(spec.find_endpoint("POST /users/").is_ok());
    assert!(spec.find_endpoint("post /users/").is_ok());

    let err = spec.find_endpoint("create_userz").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("unknown endpoint"), "{msg}");
}

#[test]
fn required_nullable_matrix_from_31_spec() {
    let spec = build(&fixture("fastapi_31.json")).unwrap();
    let endpoint = spec.find_endpoint("create_user_users__post").unwrap();
    let body = endpoint.body.as_ref().unwrap();
    assert_eq!(body.content_type, "application/json");
    let SchemaNode::Object { fields, .. } = &body.schema else {
        panic!("expected object body");
    };
    let field = |name: &str| fields.iter().find(|f| f.name == name).unwrap();

    // str -> required, not nullable
    assert!(field("email").required && !field("email").nullable);
    // Optional[str] without default -> required AND nullable
    assert!(field("nickname").required && field("nickname").nullable);
    // Optional[int] = None -> optional + nullable
    assert!(!field("age").required && field("age").nullable);
    // enum ref with default -> optional, not nullable, default preserved
    let role = field("role");
    assert!(!role.required && !role.nullable);
    assert_eq!(role.default, Some(serde_json::json!("member")));
    assert!(matches!(
        &role.schema,
        SchemaNode::String { enum_values: Some(v), .. } if v == &["admin", "member", "viewer"]
    ));
    // nested optional model
    let address = field("address");
    assert!(address.nullable && !address.required);
    assert!(matches!(address.schema, SchemaNode::Object { .. }));
    // recursive model arm degrades to Any
    let item_endpoint = spec.find_endpoint("create_item_items__post").unwrap();
    let SchemaNode::Object {
        fields: item_fields,
        ..
    } = &item_endpoint.body.as_ref().unwrap().schema
    else {
        panic!()
    };
    let parent = item_fields.iter().find(|f| f.name == "parent").unwrap();
    assert_eq!(parent.schema, SchemaNode::Any);
}

#[test]
fn params_are_normalized_with_locations() {
    let spec = build(&fixture("fastapi_31.json")).unwrap();
    let list = spec.find_endpoint("list_users_users__get").unwrap();
    let limit = list
        .params_in(ParamLocation::Query)
        .find(|p| p.name == "limit")
        .unwrap();
    assert!(!limit.required);
    assert_eq!(limit.default, Some(serde_json::json!(20)));

    let get_user = spec.find_endpoint("get_user_users__user_id__get").unwrap();
    let user_id = get_user
        .params_in(ParamLocation::Path)
        .find(|p| p.name == "user_id")
        .unwrap();
    assert!(user_id.required);
    assert!(matches!(
        &user_id.schema,
        SchemaNode::String { format: Some(f), .. } if f == "uuid"
    ));
}

#[test]
fn auth_required_follows_operation_security() {
    let spec = build(&fixture("fastapi_31.json")).unwrap();
    assert!(
        spec.find_endpoint("create_user_users__post")
            .unwrap()
            .auth_required
    );
    assert!(
        !spec
            .find_endpoint("health_health_get")
            .unwrap()
            .auth_required
    );
    assert!(
        !spec
            .find_endpoint("login_auth_login_post")
            .unwrap()
            .auth_required
    );
}

#[test]
fn template_snapshot_create_user() {
    let spec = build(&fixture("fastapi_31.json")).unwrap();
    let endpoint = spec.find_endpoint("create_user_users__post").unwrap();
    let template = build_template(endpoint);
    insta::assert_json_snapshot!(serde_json::to_value(&template).unwrap());
}

#[test]
fn template_snapshot_create_item() {
    let spec = build(&fixture("fastapi_31.json")).unwrap();
    let endpoint = spec.find_endpoint("create_item_items__post").unwrap();
    let template = build_template(endpoint);
    insta::assert_json_snapshot!(serde_json::to_value(&template).unwrap());
}

#[test]
fn openapi_30_nullable_and_allof_default() {
    let spec = build(&fixture("fastapi_30.json")).unwrap();
    assert_eq!(spec.openapi_version, "3.0.2");
    let endpoint = spec.find_endpoint("create_note_notes__post").unwrap();
    let SchemaNode::Object { fields, .. } = &endpoint.body.as_ref().unwrap().schema else {
        panic!("expected object body");
    };
    let field = |name: &str| fields.iter().find(|f| f.name == name).unwrap();

    // 3.0 `nullable: true`
    assert!(field("subtitle").nullable && !field("subtitle").required);
    // allOf single-ref + sibling default (classic FastAPI 3.0 enum-with-default)
    let priority = field("priority");
    assert_eq!(priority.default, Some(serde_json::json!("low")));
    assert!(matches!(
        &priority.schema,
        SchemaNode::String { enum_values: Some(v), .. } if v == &["low", "high"]
    ));
}

#[test]
fn login_endpoint_uses_form_content_type() {
    let spec = build(&fixture("fastapi_31.json")).unwrap();
    let login = spec.find_endpoint("login_auth_login_post").unwrap();
    assert_eq!(
        login.body.as_ref().unwrap().content_type,
        "application/x-www-form-urlencoded"
    );
}

#[test]
fn non_openapi_document_is_rejected() {
    let err = build(&serde_json::json!({"hello": "world"})).unwrap_err();
    assert!(err.to_string().contains("paths"));
}
