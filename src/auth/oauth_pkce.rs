//! 3rd-party auth: OAuth2 authorization-code flow with PKCE. Opens the
//! system browser, captures the code on a loopback listener, exchanges and
//! refreshes tokens.
//!
//! MCP/headless contexts never reach the browser path: with no usable cached
//! or refreshable token they fail with an instruction to run `hit login`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, RedirectUrl, RefreshToken,
    Scope, TokenResponse, TokenUrl,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::{AuthProvider, Interactor, StoredToken, TokenStore, token_store::now_unix};
use crate::config::OAuth2PkceConfig;
use crate::error::AuthError;

/// How long we wait for the browser to hit the loopback callback.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(180);
/// Refresh this many seconds before expiry.
const REFRESH_MARGIN_SECS: u64 = 60;

pub struct OAuth2PkceProvider {
    project: String,
    config: OAuth2PkceConfig,
    store: Box<dyn TokenStore>,
    interactor: Arc<dyn Interactor>,
    /// True in MCP/non-interactive contexts: never launch a browser.
    headless: bool,
}

impl OAuth2PkceProvider {
    pub fn new(
        project: String,
        config: OAuth2PkceConfig,
        store: Box<dyn TokenStore>,
        _client: reqwest::Client,
        interactor: Arc<dyn Interactor>,
        headless: bool,
    ) -> Self {
        // Note: the oauth2 crate carries its own reqwest (with redirects
        // disabled, as the RFC requires); the shared app client isn't reused.
        Self {
            project,
            config,
            store,
            interactor,
            headless,
        }
    }

    fn oauth_client(
        &self,
        redirect_uri: Option<String>,
    ) -> Result<
        BasicClient<
            oauth2::EndpointSet,
            oauth2::EndpointNotSet,
            oauth2::EndpointNotSet,
            oauth2::EndpointNotSet,
            oauth2::EndpointSet,
        >,
        AuthError,
    > {
        let mut client = BasicClient::new(ClientId::new(self.config.client_id.clone()))
            .set_auth_uri(
                AuthUrl::new(self.config.auth_url.to_string())
                    .map_err(|e| AuthError::OAuth(format!("auth_url: {e}")))?,
            )
            .set_token_uri(
                TokenUrl::new(self.config.token_url.to_string())
                    .map_err(|e| AuthError::OAuth(format!("token_url: {e}")))?,
            );
        if let Some(uri) = redirect_uri {
            client = client.set_redirect_uri(
                RedirectUrl::new(uri).map_err(|e| AuthError::OAuth(format!("redirect: {e}")))?,
            );
        }
        Ok(client)
    }

    fn http_client() -> Result<oauth2::reqwest::Client, AuthError> {
        // Redirects must stay disabled on the token endpoint (SSRF hardening).
        oauth2::reqwest::ClientBuilder::new()
            .redirect(oauth2::reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| AuthError::OAuth(format!("http client: {e}")))
    }

    fn store_response(
        &self,
        response: &impl TokenResponse,
        previous_refresh: Option<String>,
    ) -> Result<StoredToken, AuthError> {
        let stored = StoredToken {
            access_token: response.access_token().secret().clone(),
            refresh_token: response
                .refresh_token()
                .map(|t| t.secret().clone())
                .or(previous_refresh),
            expires_at_unix: response.expires_in().map(|d| now_unix() + d.as_secs()),
            token_type: "Bearer".into(),
        };
        self.store.save(&self.project, &stored)?;
        Ok(stored)
    }

    async fn try_refresh(&self, refresh_token: &str) -> Result<StoredToken, AuthError> {
        tracing::info!(project = self.project, "refreshing OAuth token");
        let client = self.oauth_client(None)?;
        let response = client
            .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
            .request_async(&Self::http_client()?)
            .await
            .map_err(|e| AuthError::OAuth(format!("refresh failed: {e}")))?;
        self.store_response(&response, Some(refresh_token.to_string()))
    }

    /// Full browser flow: loopback listener first, then browser.
    async fn authorization_code_flow(&self) -> Result<StoredToken, AuthError> {
        let listener = TcpListener::bind(("127.0.0.1", self.config.redirect_port))
            .await
            .map_err(|e| AuthError::OAuth(format!("binding callback listener: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| AuthError::OAuth(e.to_string()))?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");

        let client = self.oauth_client(Some(redirect_uri))?;
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let (auth_url, csrf_state) = client
            .authorize_url(CsrfToken::new_random)
            .add_scopes(self.config.scopes.iter().map(|s| Scope::new(s.clone())))
            .set_pkce_challenge(pkce_challenge)
            .url();

        self.interactor.notify(&format!(
            "Opening browser for OAuth login. If nothing happens, open:\n  {auth_url}"
        ));
        // HITPOINT_NO_BROWSER: print the URL only (SSH sessions, tests).
        if std::env::var_os("HITPOINT_NO_BROWSER").is_none()
            && let Err(e) = open::that(auth_url.as_str())
        {
            tracing::warn!(error = %e, "failed to launch browser; user must open the URL manually");
        }

        let (code, state) = tokio::time::timeout(CALLBACK_TIMEOUT, accept_callback(&listener))
            .await
            .map_err(|_| {
                AuthError::OAuth(format!(
                    "timed out after {}s waiting for the OAuth callback",
                    CALLBACK_TIMEOUT.as_secs()
                ))
            })??;

        if state != *csrf_state.secret() {
            return Err(AuthError::OAuth(
                "state mismatch in OAuth callback — possible CSRF; aborting".into(),
            ));
        }

        let response = client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(pkce_verifier)
            .request_async(&Self::http_client()?)
            .await
            .map_err(|e| AuthError::OAuth(format!("code exchange failed: {e}")))?;

        tracing::info!(project = self.project, "OAuth login complete");
        self.store_response(&response, None)
    }
}

#[async_trait]
impl AuthProvider for OAuth2PkceProvider {
    async fn token(&self) -> Result<String, AuthError> {
        let stored = self.store.load(&self.project);

        if let Some(token) = &stored
            && token.is_fresh(REFRESH_MARGIN_SECS)
        {
            return Ok(token.access_token.clone());
        }

        if let Some(refresh_token) = stored.as_ref().and_then(|t| t.refresh_token.clone()) {
            match self.try_refresh(&refresh_token).await {
                Ok(token) => return Ok(token.access_token),
                Err(e) => tracing::warn!(
                    project = self.project,
                    error = %e,
                    "token refresh failed; falling back to full re-auth"
                ),
            }
        }

        if self.headless {
            return Err(AuthError::InteractionRequired(format!(
                "project '{}' uses browser-based OAuth — run `hit login {}` in a terminal",
                self.project, self.project
            )));
        }

        Ok(self.authorization_code_flow().await?.access_token)
    }

    async fn invalidate(&self) {
        // Keep the refresh token: a 401 usually means the access token aged
        // out; the next token() call will refresh, or fully re-auth if that
        // fails too.
        if let Some(mut stored) = self.store.load(&self.project)
            && stored.refresh_token.is_some()
        {
            stored.expires_at_unix = Some(0);
            let _ = self.store.save(&self.project, &stored);
            return;
        }
        if let Err(e) = self.store.clear(&self.project) {
            tracing::warn!(project = self.project, error = %e, "failed to clear token");
        }
    }

    fn supports_headless(&self) -> bool {
        false
    }

    fn cached_expiry(&self) -> Option<u64> {
        self.store.load(&self.project)?.expires_at_unix
    }
}

/// Accept one connection and pull `code` and `state` out of the request line.
async fn accept_callback(listener: &TcpListener) -> Result<(String, String), AuthError> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|e| AuthError::OAuth(format!("callback accept: {e}")))?;

        let mut buf = vec![0u8; 8192];
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| AuthError::OAuth(format!("callback read: {e}")))?;
        let request = String::from_utf8_lossy(&buf[..n]);

        let Some(path) = request.split_whitespace().nth(1) else {
            continue; // malformed; keep listening until the timeout fires
        };
        // Browsers also ask for /favicon.ico etc.
        if !path.starts_with("/callback") {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n").await;
            continue;
        }

        let parsed = url::Url::parse(&format!("http://localhost{path}"))
            .map_err(|e| AuthError::OAuth(format!("callback URL: {e}")))?;
        let get = |key: &str| {
            parsed
                .query_pairs()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.into_owned())
        };

        if let Some(error) = get("error") {
            let description = get("error_description").unwrap_or_default();
            let _ = respond_html(&mut stream, "Login failed — you can close this tab.").await;
            return Err(AuthError::OAuth(format!(
                "authorization server returned '{error}': {description}"
            )));
        }

        match (get("code"), get("state")) {
            (Some(code), Some(state)) => {
                let _ = respond_html(
                    &mut stream,
                    "hitpoint: login complete — you can close this tab.",
                )
                .await;
                return Ok((code, state));
            }
            _ => {
                let _ = respond_html(&mut stream, "Missing code/state in callback.").await;
                // keep listening
            }
        }
    }
}

async fn respond_html(stream: &mut tokio::net::TcpStream, body: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}
