//! End-to-end CLI tests: real binary, temp config, wiremock backend.

use assert_cmd::Command;
use base64::Engine;
use serde_json::Value;
use tempfile::TempDir;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture_json() -> Value {
    let raw = std::fs::read_to_string("tests/fixtures/fastapi_31.json").unwrap();
    serde_json::from_str(&raw).unwrap()
}

/// Write a projects.toml in a tempdir and return (dir, config_path).
fn write_config(contents: &str) -> (TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("projects.toml");
    std::fs::write(&path, contents).unwrap();
    (dir, path)
}

fn hit() -> Command {
    Command::cargo_bin("hitpoint").unwrap()
}

fn parse_envelope(output: &[u8]) -> Value {
    serde_json::from_slice(output).expect("stdout should be a JSON envelope")
}

async fn mock_server_with_spec() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/openapi.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture_json()))
        .mount(&server)
        .await;
    server
}

fn make_jwt(exp: u64) -> String {
    let engine = &base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header = engine.encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = engine.encode(format!(r#"{{"sub":"admin","exp":{exp}}}"#));
    format!("{header}.{payload}.")
}

fn far_future() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 86_400
}

// ---------------------------------------------------------------- projects

#[test]
fn projects_add_list_remove_round_trip() {
    let (dir, config) = write_config("");

    hit()
        .args(["--config", config.to_str().unwrap(), "--json"])
        .args([
            "projects",
            "add",
            "demo",
            "--base-url",
            "http://localhost:9",
        ])
        .assert()
        .success();

    let output = hit()
        .args([
            "--config",
            config.to_str().unwrap(),
            "--json",
            "projects",
            "list",
        ])
        .output()
        .unwrap();
    let envelope = parse_envelope(&output.stdout);
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["data"][0]["name"], "demo");

    hit()
        .args([
            "--config",
            config.to_str().unwrap(),
            "--json",
            "projects",
            "remove",
            "demo",
        ])
        .assert()
        .success();
    drop(dir);
}

#[test]
fn unknown_project_exits_1_with_kind() {
    let (_dir, config) = write_config("");
    let output = hit()
        .args([
            "--config",
            config.to_str().unwrap(),
            "--json",
            "tags",
            "ghost",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let envelope = parse_envelope(&output.stdout);
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["kind"], "unknown_project");
}

// ------------------------------------------------------------- spec reads

#[tokio::test(flavor = "multi_thread")]
async fn tags_endpoints_template_from_live_spec() {
    let server = mock_server_with_spec().await;
    let (_dir, config) = write_config(&format!(
        "[projects.demo]\nbase_url = \"{}\"\n",
        server.uri()
    ));
    let config_arg = config.to_str().unwrap().to_string();

    let assertions = tokio::task::spawn_blocking(move || {
        let output = hit()
            .args(["--config", &config_arg, "--json", "tags", "demo"])
            .output()
            .unwrap();
        let envelope = parse_envelope(&output.stdout);
        assert_eq!(envelope["ok"], true);
        let tags: Vec<&str> = envelope["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(tags, vec!["users", "items", "auth", "untagged"]);

        let output = hit()
            .args([
                "--config",
                &config_arg,
                "--json",
                "endpoints",
                "demo",
                "--tag",
                "users",
            ])
            .output()
            .unwrap();
        let envelope = parse_envelope(&output.stdout);
        assert_eq!(envelope["data"].as_array().unwrap().len(), 4);

        let output = hit()
            .args([
                "--config",
                &config_arg,
                "--json",
                "endpoints",
                "demo",
                "--search",
                "health",
            ])
            .output()
            .unwrap();
        let envelope = parse_envelope(&output.stdout);
        assert_eq!(envelope["data"].as_array().unwrap().len(), 1);

        let output = hit()
            .args([
                "--config",
                &config_arg,
                "--json",
                "template",
                "demo",
                "POST /users/",
            ])
            .output()
            .unwrap();
        let envelope = parse_envelope(&output.stdout);
        assert_eq!(envelope["data"]["endpoint_id"], "create_user_users__post");
        assert_eq!(envelope["data"]["body"]["role"], "member");
        assert!(
            envelope["data"]["nullable_paths"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p == "nickname")
        );
    });
    assertions.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn spec_file_fallback_when_server_down() {
    // base_url points nowhere; spec_file carries the fixture.
    let (_dir, config) = write_config(&format!(
        "[projects.offline]\nbase_url = \"http://127.0.0.1:1\"\nspec_file = \"{}/tests/fixtures/fastapi_31.json\"\n",
        env!("CARGO_MANIFEST_DIR")
    ));
    let config_arg = config.to_str().unwrap().to_string();

    tokio::task::spawn_blocking(move || {
        let output = hit()
            .args(["--config", &config_arg, "--json", "tags", "offline"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let envelope = parse_envelope(&output.stdout);
        assert_eq!(envelope["ok"], true);
    })
    .await
    .unwrap();
}

// ---------------------------------------------------------------- hit run

#[tokio::test(flavor = "multi_thread")]
async fn run_post_with_body_query_and_path_params() {
    let server = mock_server_with_spec().await;
    Mock::given(method("PATCH"))
        .and(path("/users/u-1"))
        .and(body_string_contains("\"name\":\"Neo\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": "u-1"})))
        .mount(&server)
        .await;

    let (_dir, config) = write_config(&format!(
        "[projects.demo]\nbase_url = \"{}\"\n",
        server.uri()
    ));
    let config_arg = config.to_str().unwrap().to_string();

    tokio::task::spawn_blocking(move || {
        let output = hit()
            .args([
                "--config",
                &config_arg,
                "--json",
                "run",
                "demo",
                "update_user_users__user_id__patch",
                "-p",
                "user_id=u-1",
                "--body",
                r#"{"name": "Neo"}"#,
            ])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let envelope = parse_envelope(&output.stdout);
        assert_eq!(envelope["data"]["status"], 200);
        assert_eq!(envelope["data"]["body"]["id"], "u-1");
        assert!(envelope["data"]["latency_ms"].is_u64());
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn run_422_maps_to_exit_5_and_allow_error_to_0() {
    let server = mock_server_with_spec().await;
    Mock::given(method("POST"))
        .and(path("/items/"))
        .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
            "detail": [{"loc": ["body", "sku"], "msg": "field required", "type": "missing"}]
        })))
        .mount(&server)
        .await;

    let (_dir, config) = write_config(&format!(
        "[projects.demo]\nbase_url = \"{}\"\n",
        server.uri()
    ));
    let config_arg = config.to_str().unwrap().to_string();

    tokio::task::spawn_blocking(move || {
        let output = hit()
            .args([
                "--config",
                &config_arg,
                "--json",
                "run",
                "demo",
                "create_item_items__post",
                "--body",
                "{}",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(5));
        let envelope = parse_envelope(&output.stdout);
        assert_eq!(
            envelope["ok"], true,
            "HTTP errors are successful invocations"
        );
        assert_eq!(envelope["data"]["status"], 422);

        let output = hit()
            .args([
                "--config",
                &config_arg,
                "--json",
                "run",
                "demo",
                "create_item_items__post",
                "--body",
                "{}",
                "--allow-error",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_endpoint_exits_2_with_suggestions() {
    let server = mock_server_with_spec().await;
    let (_dir, config) = write_config(&format!(
        "[projects.demo]\nbase_url = \"{}\"\n",
        server.uri()
    ));
    let config_arg = config.to_str().unwrap().to_string();

    tokio::task::spawn_blocking(move || {
        let output = hit()
            .args([
                "--config",
                &config_arg,
                "--json",
                "template",
                "demo",
                "create_user",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let envelope = parse_envelope(&output.stdout);
        assert_eq!(envelope["error"]["kind"], "unknown_endpoint");
        assert!(
            envelope["error"]["message"]
                .as_str()
                .unwrap()
                .contains("create_user_users__post")
        );
    })
    .await
    .unwrap();
}

// ------------------------------------------------------------------- auth

fn jwt_config(server_uri: &str) -> String {
    format!(
        r#"
# Tests must never touch the host OS keyring.
[settings]
token_store = "file"

[projects.demo]
base_url = "{server_uri}"

[projects.demo.auth]
type = "jwt_login"
login_path = "/auth/login"
username = {{ env = "HIT_TEST_USER" }}
password = {{ env = "HIT_TEST_PASS" }}
"#
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn jwt_login_attaches_bearer_token() {
    let server = mock_server_with_spec().await;
    let token = make_jwt(far_future());

    Mock::given(method("POST"))
        .and(path("/auth/login"))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string_contains("username=admin"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"access_token": token, "token_type": "bearer"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/"))
        .and(header("authorization", format!("Bearer {token}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .expect(2)
        .mount(&server)
        .await;

    let (_dir, config) = write_config(&jwt_config(&server.uri()));
    let config_arg = config.to_str().unwrap().to_string();

    tokio::task::spawn_blocking(move || {
        // Two runs: the second must reuse the cached token (login expect(1)).
        for _ in 0..2 {
            let output = hit()
                .env("HIT_TEST_USER", "admin")
                .env("HIT_TEST_PASS", "secret")
                .args([
                    "--config",
                    &config_arg,
                    "--json",
                    "run",
                    "demo",
                    "list_users_users__get",
                ])
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(0),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    })
    .await
    .unwrap();
    // MockServer verifies expect() counts on drop.
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_token_triggers_401_retry_with_relogin() {
    let server = mock_server_with_spec().await;
    let fresh = make_jwt(far_future());

    Mock::given(method("POST"))
        .and(path("/auth/login"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"access_token": fresh, "token_type": "bearer"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    // Stale opaque token (no exp -> treated as fresh until the server says 401).
    Mock::given(method("GET"))
        .and(path("/users/"))
        .and(header("authorization", "Bearer stale-opaque-token"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/"))
        .and(header("authorization", format!("Bearer {fresh}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .expect(1)
        .mount(&server)
        .await;

    let (dir, config) = write_config(&jwt_config(&server.uri()));
    // Pre-seed the token store with the stale token.
    let token_dir = dir.path().join("tokens");
    std::fs::create_dir_all(&token_dir).unwrap();
    std::fs::write(
        token_dir.join("demo.json"),
        r#"{"access_token": "stale-opaque-token", "token_type": "Bearer"}"#,
    )
    .unwrap();
    let config_arg = config.to_str().unwrap().to_string();

    tokio::task::spawn_blocking(move || {
        let output = hit()
            .env("HIT_TEST_USER", "admin")
            .env("HIT_TEST_PASS", "secret")
            .args([
                "--config",
                &config_arg,
                "--json",
                "run",
                "demo",
                "list_users_users__get",
            ])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let envelope = parse_envelope(&output.stdout);
        assert_eq!(envelope["data"]["status"], 200);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn login_and_logout_commands() {
    let server = mock_server_with_spec().await;
    let token = make_jwt(far_future());
    Mock::given(method("POST"))
        .and(path("/auth/login"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"access_token": token, "token_type": "bearer"})),
        )
        .mount(&server)
        .await;

    let (dir, config) = write_config(&jwt_config(&server.uri()));
    let config_arg = config.to_str().unwrap().to_string();
    let token_file = dir.path().join("tokens/demo.json");

    tokio::task::spawn_blocking(move || {
        let output = hit()
            .env("HIT_TEST_USER", "admin")
            .env("HIT_TEST_PASS", "secret")
            .args(["--config", &config_arg, "--json", "login", "demo"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let envelope = parse_envelope(&output.stdout);
        assert!(envelope["data"]["expires_at_unix"].is_u64());
        assert!(token_file.exists());

        hit()
            .args(["--config", &config_arg, "--json", "logout", "demo"])
            .assert()
            .success();
        assert!(!token_file.exists());
    })
    .await
    .unwrap();
}
