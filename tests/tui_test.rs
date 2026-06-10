//! TUI smoke tests: render screens against a TestBackend and drive the form
//! with key events. Catches draw-time panics and layout regressions.

use std::collections::HashMap;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tokio::sync::mpsc;

use hitpoint::AppServices;
use hitpoint::config::{Paths, ProjectsConfig};
use hitpoint::spec::{SpecOrigin, build};
use hitpoint::tui::screens::{Action, Screen, endpoints::EndpointList, form::RequestForm};
use hitpoint::tui::{AppCtx, SpecBundle};

fn fixture_bundle() -> Arc<SpecBundle> {
    let raw = std::fs::read_to_string("tests/fixtures/fastapi_31.json").unwrap();
    let doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
    Arc::new(SpecBundle {
        project: "demo".into(),
        spec: build(&doc).unwrap(),
        origin: SpecOrigin::File,
    })
}

fn test_ctx() -> AppCtx {
    let dir = std::env::temp_dir().join(format!("hitpoint-tui-test-{}", std::process::id()));
    let config_file = dir.join("projects.toml");
    let mut config = ProjectsConfig::default();
    config.projects.insert(
        "demo".into(),
        toml::from_str::<hitpoint::config::ProjectConfig>("base_url = \"http://localhost:1\"")
            .unwrap(),
    );
    let paths = Paths::resolve(Some(&config_file)).unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    AppCtx {
        services: Arc::new(AppServices::new(paths, config, None)),
        tx,
        specs: HashMap::new(),
        modal: None,
        status: None,
        request_seq: 0,
        frame: 0,
    }
}

fn draw(screen: &mut dyn Screen, ctx: &AppCtx) -> String {
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| screen.draw(frame, frame.area(), ctx))
        .unwrap();
    format!("{:?}", terminal.backend().buffer())
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn endpoint_list_renders_and_filters() {
    let ctx = &mut test_ctx();
    let mut screen = EndpointList::new(fixture_bundle(), Some("users".into()));
    let rendered = draw(&mut screen, ctx);
    assert!(rendered.contains("/users/{user_id}"));

    // Filter down to PATCH.
    screen.handle_key(key(KeyCode::Char('/')), ctx);
    for c in "update".chars() {
        screen.handle_key(key(KeyCode::Char(c)), ctx);
    }
    screen.handle_key(key(KeyCode::Enter), ctx);
    let rendered = draw(&mut screen, ctx);
    assert!(rendered.contains("PATCH"));
    assert!(!rendered.contains("POST"));
}

#[test]
fn form_renders_states_and_cycles_with_keys() {
    let ctx = &mut test_ctx();
    let bundle = fixture_bundle();
    let endpoint = bundle
        .spec
        .find_endpoint("create_user_users__post")
        .unwrap()
        .clone();
    let mut screen = RequestForm::new(bundle, endpoint);

    let rendered = draw(&mut screen, ctx);
    assert!(rendered.contains("email"));
    assert!(rendered.contains("⊘ excluded")); // address / age
    assert!(rendered.contains("◂ member ▸")); // enum default

    // Type into the first field (email) via the inline editor.
    screen.handle_key(key(KeyCode::Enter), ctx);
    for c in "a@b.c".chars() {
        screen.handle_key(key(KeyCode::Char(c)), ctx);
    }
    screen.handle_key(key(KeyCode::Enter), ctx);
    let rendered = draw(&mut screen, ctx);
    assert!(rendered.contains("a@b.c"));

    // Shift+X on email (required, not nullable) -> status hint, no change.
    screen.handle_key(key(KeyCode::Char('X')), ctx);
    assert!(ctx.status.as_deref().unwrap_or("").contains("required"));

    // Move to nickname (required + nullable) and null it.
    screen.handle_key(key(KeyCode::Down), ctx);
    screen.handle_key(key(KeyCode::Down), ctx);
    screen.handle_key(key(KeyCode::Char('X')), ctx);
    let rendered = draw(&mut screen, ctx);
    assert!(rendered.contains("∅ null"));
}

#[test]
fn form_submit_blocks_on_missing_required_field() {
    let ctx = &mut test_ctx();
    let bundle = fixture_bundle();
    let endpoint = bundle
        .spec
        .find_endpoint("create_user_users__post")
        .unwrap()
        .clone();
    let mut screen = RequestForm::new(bundle, endpoint);

    let action = screen.handle_key(
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        ctx,
    );
    assert!(matches!(action, Action::None));
    assert!(ctx.status.as_deref().unwrap_or("").contains("email"));
}

#[test]
fn response_view_renders_422_detail() {
    use hitpoint::http::ApiResponse;
    use hitpoint::tui::AppMsg;
    use hitpoint::tui::screens::response::ResponseView;

    let ctx = &mut test_ctx();
    let mut screen = ResponseView::loading(1, fixture_bundle(), "POST".into(), "/users/".into());
    let rendered = draw(&mut screen, ctx);
    assert!(rendered.contains("loading"));

    let response = ApiResponse {
        status: 422,
        headers: vec![("content-type".into(), "application/json".into())],
        body: serde_json::json!({
            "detail": [{"loc": ["body", "email"], "msg": "field required", "type": "missing"}]
        }),
        body_is_json: true,
        latency_ms: 12,
        url: "http://localhost/users/".into(),
        method: "POST".into(),
    };
    screen.handle_msg(
        &AppMsg::Response {
            request_seq: 1,
            result: Ok(response),
        },
        ctx,
    );
    let rendered = draw(&mut screen, ctx);
    assert!(rendered.contains("422"));
    assert!(rendered.contains("body.email: field required"));
}

#[test]
fn endpoint_list_shows_docs_for_hovered_endpoint() {
    let ctx = &mut test_ctx();
    let mut screen = EndpointList::new(fixture_bundle(), Some("users".into()));
    // Move to POST /users/ (second row) and render.
    screen.handle_key(key(KeyCode::Down), ctx);
    let rendered = draw(&mut screen, ctx);
    assert!(rendered.contains("Create a new user account."));
    assert!(rendered.contains("201"));
    assert!(rendered.contains("User created"));
    assert!(rendered.contains("<string:uuid>")); // example 201 body
}

#[test]
fn form_docs_toggle_shows_description_and_response() {
    let ctx = &mut test_ctx();
    let bundle = fixture_bundle();
    let endpoint = bundle
        .spec
        .find_endpoint("create_user_users__post")
        .unwrap()
        .clone();
    let mut screen = RequestForm::new(bundle, endpoint);

    let rendered = draw(&mut screen, ctx);
    assert!(!rendered.contains("example 201 body"));

    screen.handle_key(key(KeyCode::Char('i')), ctx);
    let rendered = draw(&mut screen, ctx);
    assert!(rendered.contains("Create a new user account."));
    assert!(rendered.contains("example 201 body"));
    assert!(rendered.contains("<string:email>"));
}
