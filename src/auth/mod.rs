//! Authentication: per-project providers behind one trait, a token store,
//! and the manager that owns the attach/401-retry policy.

pub mod jwt_login;
pub mod oauth_pkce;
pub mod token_store;

use std::sync::Arc;

use async_trait::async_trait;

use crate::config::{AuthConfig, Paths, ProjectConfig, Settings};
use crate::error::AuthError;

pub use token_store::{StoredToken, TokenStore, new_token_store};

/// A source of bearer tokens. Implementations cache via `TokenStore` and
/// only hit the network when the cache is missing or stale.
#[async_trait]
pub trait AuthProvider: Send + Sync {
    /// Return a valid bearer token, logging in / refreshing as needed.
    async fn token(&self) -> Result<String, AuthError>;
    /// Drop any cached token (called after a 401).
    async fn invalidate(&self);
    /// False when acquiring a token needs a browser or a human prompt.
    fn supports_headless(&self) -> bool;
    /// Expiry of the currently cached token, if known (unix seconds).
    fn cached_expiry(&self) -> Option<u64>;
}

/// How providers ask the human for input. CLI prompts on the terminal; MCP
/// mode denies with an instructive error; the TUI supplies a modal-backed impl.
pub trait Interactor: Send + Sync {
    fn prompt_line(&self, label: &str) -> Result<String, AuthError>;
    fn prompt_secret(&self, label: &str) -> Result<String, AuthError>;
    fn notify(&self, message: &str);
}

pub struct CliInteractor;

impl Interactor for CliInteractor {
    fn prompt_line(&self, label: &str) -> Result<String, AuthError> {
        eprint!("{label}: ");
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| AuthError::Credential(format!("reading stdin: {e}")))?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }

    fn prompt_secret(&self, label: &str) -> Result<String, AuthError> {
        rpassword::prompt_password(format!("{label}: "))
            .map_err(|e| AuthError::Credential(format!("reading password: {e}")))
    }

    fn notify(&self, message: &str) {
        eprintln!("{message}");
    }
}

/// Refuses all interaction — used by MCP mode and other headless contexts.
pub struct DenyInteractor {
    pub instruction: String,
}

impl Interactor for DenyInteractor {
    fn prompt_line(&self, _label: &str) -> Result<String, AuthError> {
        Err(AuthError::InteractionRequired(self.instruction.clone()))
    }

    fn prompt_secret(&self, _label: &str) -> Result<String, AuthError> {
        Err(AuthError::InteractionRequired(self.instruction.clone()))
    }

    fn notify(&self, message: &str) {
        tracing::info!(message, "auth notification (headless)");
    }
}

/// Owns the provider for one project and serializes token operations so
/// concurrent requests don't trigger duplicate logins.
pub struct AuthManager {
    provider: Box<dyn AuthProvider>,
    lock: tokio::sync::Mutex<()>,
}

impl AuthManager {
    /// Build the manager for a project, or `None` when no auth is configured.
    pub fn for_project(
        project_name: &str,
        project: &ProjectConfig,
        settings: &Settings,
        paths: &Paths,
        client: reqwest::Client,
        interactor: Arc<dyn Interactor>,
        headless: bool,
    ) -> Result<Option<Self>, AuthError> {
        let Some(auth) = &project.auth else {
            return Ok(None);
        };
        let store = new_token_store(settings.token_store, paths.token_dir.clone())?;
        let provider: Box<dyn AuthProvider> = match auth {
            AuthConfig::JwtLogin(config) => Box::new(jwt_login::JwtLoginProvider::new(
                project_name.to_string(),
                project.base_url.clone(),
                config.clone(),
                store,
                client,
                interactor,
            )),
            AuthConfig::Oauth2Pkce(config) => Box::new(oauth_pkce::OAuth2PkceProvider::new(
                project_name.to_string(),
                config.clone(),
                store,
                client,
                interactor,
                headless,
            )),
        };
        Ok(Some(Self {
            provider,
            lock: tokio::sync::Mutex::new(()),
        }))
    }

    pub async fn bearer(&self) -> Result<String, AuthError> {
        let _guard = self.lock.lock().await;
        self.provider.token().await
    }

    pub async fn invalidate(&self) {
        let _guard = self.lock.lock().await;
        self.provider.invalidate().await;
    }

    pub fn cached_expiry(&self) -> Option<u64> {
        self.provider.cached_expiry()
    }

    pub fn supports_headless(&self) -> bool {
        self.provider.supports_headless()
    }
}

/// Resolve a configured credential reference to its value.
pub(crate) fn resolve_credential(
    cred: &crate::config::CredentialRef,
    label: &str,
    interactor: &dyn Interactor,
    secret: bool,
) -> Result<String, AuthError> {
    use crate::config::CredentialRef;
    match cred {
        CredentialRef::Value { value } => Ok(value.clone()),
        CredentialRef::Env { env } => std::env::var(env)
            .map_err(|_| AuthError::Credential(format!("environment variable {env} is not set"))),
        CredentialRef::Keyring { keyring } => keyring_lookup(keyring),
        CredentialRef::Prompt { prompt } => {
            if !prompt {
                return Err(AuthError::Credential(format!(
                    "{label}: prompt = false makes the credential unreachable"
                )));
            }
            if secret {
                interactor.prompt_secret(label)
            } else {
                interactor.prompt_line(label)
            }
        }
    }
}

#[cfg(feature = "keyring")]
fn keyring_lookup(entry_name: &str) -> Result<String, AuthError> {
    let entry = keyring::Entry::new("hitpoint", entry_name)
        .map_err(|e| AuthError::Credential(format!("keyring: {e}")))?;
    entry
        .get_password()
        .map_err(|e| AuthError::Credential(format!("keyring entry '{entry_name}': {e}")))
}

#[cfg(not(feature = "keyring"))]
fn keyring_lookup(entry_name: &str) -> Result<String, AuthError> {
    Err(AuthError::Credential(format!(
        "credential '{entry_name}' uses the keyring, but this build lacks the 'keyring' feature"
    )))
}
