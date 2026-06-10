//! Serde structs mirroring `projects.toml`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectsConfig {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub projects: BTreeMap<String, ProjectConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Settings {
    /// Re-fetch openapi.json from the live server after this many seconds.
    pub spec_cache_ttl_secs: u64,
    /// Default HTTP timeout for executed requests.
    pub timeout_secs: u64,
    /// Where tokens are persisted: "auto" (keyring then file), "keyring", "file".
    pub token_store: TokenStoreKind,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            spec_cache_ttl_secs: 300,
            timeout_secs: 30,
            token_store: TokenStoreKind::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenStoreKind {
    Auto,
    Keyring,
    File,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub base_url: Url,
    /// Optional on-disk openapi.json used when the live server is unreachable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub default_headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthConfig>,
    /// Framework adapter; only FastAPI today.
    #[serde(default)]
    pub framework: Framework,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Framework {
    #[default]
    Fastapi,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthConfig {
    JwtLogin(JwtLoginConfig),
    Oauth2Pkce(OAuth2PkceConfig),
}

impl AuthConfig {
    pub fn type_name(&self) -> &'static str {
        match self {
            AuthConfig::JwtLogin(_) => "jwt_login",
            AuthConfig::Oauth2Pkce(_) => "oauth2_pkce",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JwtLoginConfig {
    /// POSTed relative to the project base_url, e.g. "/auth/login".
    pub login_path: String,
    /// "form" matches FastAPI's OAuth2PasswordRequestForm; "json" posts a JSON object.
    #[serde(default)]
    pub login_content_type: LoginContentType,
    pub username: CredentialRef,
    pub password: CredentialRef,
    /// JSON pointer into the login response locating the token.
    #[serde(default = "default_token_pointer")]
    pub token_json_pointer: String,
    /// Re-login this many seconds before the JWT `exp`.
    #[serde(default = "default_refresh_margin")]
    pub refresh_margin_secs: u64,
}

fn default_token_pointer() -> String {
    "/access_token".to_string()
}

fn default_refresh_margin() -> u64 {
    60
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginContentType {
    #[default]
    Form,
    Json,
}

/// Where a credential value comes from. Spelled in TOML as
/// `{ env = "VAR" }`, `{ value = "literal" }`, `{ keyring = "entry" }`, or `{ prompt = true }`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum CredentialRef {
    Env { env: String },
    Value { value: String },
    Keyring { keyring: String },
    Prompt { prompt: bool },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OAuth2PkceConfig {
    pub auth_url: Url,
    pub token_url: Url,
    pub client_id: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// 0 = pick an ephemeral port; some IdPs require a pre-registered fixed port.
    #[serde(default)]
    pub redirect_port: u16,
}
