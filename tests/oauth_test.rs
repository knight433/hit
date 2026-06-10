//! OAuth2 PKCE flow tests: full authorization-code round trip (driving the
//! loopback callback manually instead of a browser) and the refresh path.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use hitpoint::auth::oauth_pkce::OAuth2PkceProvider;
use hitpoint::auth::{AuthProvider, Interactor, StoredToken, new_token_store};
use hitpoint::config::{OAuth2PkceConfig, TokenStoreKind};
use hitpoint::error::AuthError;

/// Captures notify() messages so the test can fish out the auth URL.
struct CapturingInteractor {
    messages: Mutex<Vec<String>>,
}

impl Interactor for CapturingInteractor {
    fn prompt_line(&self, _label: &str) -> Result<String, AuthError> {
        Err(AuthError::Credential("no prompts in this test".into()))
    }
    fn prompt_secret(&self, _label: &str) -> Result<String, AuthError> {
        Err(AuthError::Credential("no prompts in this test".into()))
    }
    fn notify(&self, message: &str) {
        self.messages.lock().unwrap().push(message.to_string());
    }
}

fn pkce_config(server_uri: &str) -> OAuth2PkceConfig {
    toml::from_str(&format!(
        r#"
        auth_url = "{server_uri}/authorize"
        token_url = "{server_uri}/token"
        client_id = "hitpoint-test"
        scopes = ["openid", "api:read"]
        redirect_port = 0
        "#
    ))
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn full_pkce_flow_with_manual_callback_then_cache_then_refresh() {
    // SAFETY: test-local env tweak before any threads read it.
    unsafe { std::env::set_var("HITPOINT_NO_BROWSER", "1") };

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code_verifier="))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-1",
            "refresh_token": "refresh-1",
            "token_type": "bearer",
            "expires_in": 3600,
        })))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let store = new_token_store(TokenStoreKind::File, dir.path().to_path_buf()).unwrap();
    let interactor = Arc::new(CapturingInteractor {
        messages: Mutex::new(Vec::new()),
    });
    let provider = Arc::new(OAuth2PkceProvider::new(
        "crm".into(),
        pkce_config(&server.uri()),
        store,
        reqwest::Client::new(),
        interactor.clone(),
        false,
    ));

    // Kick off the flow; it blocks waiting for the callback.
    let flow = tokio::spawn({
        let provider = provider.clone();
        async move { provider.token().await }
    });

    // Wait for the auth URL to be announced, then extract redirect_uri+state.
    let auth_url = {
        let mut found = None;
        for _ in 0..100 {
            if let Some(message) = interactor.messages.lock().unwrap().last()
                && let Some(start) = message.find("http")
            {
                found = Some(message[start..].trim().to_string());
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        found.expect("auth URL should be announced")
    };
    let parsed = url::Url::parse(&auth_url).unwrap();
    let query = |key: &str| {
        parsed
            .query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
            .unwrap()
    };
    assert_eq!(query("response_type"), "code");
    assert_eq!(query("code_challenge_method"), "S256");
    assert!(query("scope").contains("api:read"));

    // Play the browser: hit the loopback callback with code + state.
    let callback = format!(
        "{}?code=test-code&state={}",
        query("redirect_uri"),
        query("state")
    );
    let response = reqwest::get(&callback).await.unwrap();
    assert!(response.status().is_success());

    let token = flow.await.unwrap().unwrap();
    assert_eq!(token, "access-1");

    // Second call: served from the store, no network.
    assert_eq!(provider.token().await.unwrap(), "access-1");

    // Refresh path: invalidate (keeps refresh token), expect refresh grant.
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=refresh-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-2",
            "token_type": "bearer",
            "expires_in": 3600,
        })))
        .expect(1)
        .mount(&server)
        .await;

    provider.invalidate().await;
    assert_eq!(provider.token().await.unwrap(), "access-2");
}

#[tokio::test(flavor = "multi_thread")]
async fn state_mismatch_aborts_flow() {
    unsafe { std::env::set_var("HITPOINT_NO_BROWSER", "1") };

    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let store = new_token_store(TokenStoreKind::File, dir.path().to_path_buf()).unwrap();
    let interactor = Arc::new(CapturingInteractor {
        messages: Mutex::new(Vec::new()),
    });
    let provider = Arc::new(OAuth2PkceProvider::new(
        "crm".into(),
        pkce_config(&server.uri()),
        store,
        reqwest::Client::new(),
        interactor.clone(),
        false,
    ));

    let flow = tokio::spawn({
        let provider = provider.clone();
        async move { provider.token().await }
    });

    let auth_url = {
        let mut found = None;
        for _ in 0..100 {
            if let Some(message) = interactor.messages.lock().unwrap().last()
                && let Some(start) = message.find("http")
            {
                found = Some(message[start..].trim().to_string());
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        found.expect("auth URL should be announced")
    };
    let parsed = url::Url::parse(&auth_url).unwrap();
    let redirect_uri = parsed
        .query_pairs()
        .find(|(k, _)| k == "redirect_uri")
        .map(|(_, v)| v.into_owned())
        .unwrap();

    let _ = reqwest::get(format!("{redirect_uri}?code=x&state=WRONG")).await;
    let result = flow.await.unwrap();
    assert!(matches!(result, Err(AuthError::OAuth(message)) if message.contains("state mismatch")));
}

#[tokio::test(flavor = "multi_thread")]
async fn headless_mode_never_opens_a_flow() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let store = new_token_store(TokenStoreKind::File, dir.path().to_path_buf()).unwrap();
    // Seed an expired token without a refresh token.
    store
        .save(
            "crm",
            &StoredToken {
                access_token: "stale".into(),
                refresh_token: None,
                expires_at_unix: Some(1),
                token_type: "Bearer".into(),
            },
        )
        .unwrap();
    let provider = OAuth2PkceProvider::new(
        "crm".into(),
        pkce_config(&server.uri()),
        store,
        reqwest::Client::new(),
        Arc::new(CapturingInteractor {
            messages: Mutex::new(Vec::new()),
        }),
        true, // headless
    );
    let result = provider.token().await;
    assert!(matches!(result, Err(AuthError::InteractionRequired(m)) if m.contains("hit login")));
}
