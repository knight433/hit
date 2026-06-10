//! Error taxonomy shared by the CLI envelope, MCP error results, and exit codes.

use thiserror::Error;

/// Exit codes for the `hit` binary.
pub mod exit_code {
    pub const OK: i32 = 0;
    pub const USAGE: i32 = 1;
    pub const SPEC: i32 = 2;
    pub const AUTH: i32 = 3;
    pub const NETWORK: i32 = 4;
    pub const HTTP_4XX: i32 = 5;
    pub const HTTP_5XX: i32 = 6;
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file not found at {0}")]
    NotFound(String),
    #[error("failed to read config {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid config: {0}")]
    Parse(String),
    #[error("unknown project '{name}'{}", suggest(.available))]
    UnknownProject {
        name: String,
        available: Vec<String>,
    },
    #[error("project '{0}' already exists")]
    DuplicateProject(String),
    #[error("invalid value for {field}: {message}")]
    Invalid { field: String, message: String },
}

#[derive(Debug, Error)]
pub enum SpecError {
    #[error("failed to fetch OpenAPI spec from {url}: {message}")]
    Fetch { url: String, message: String },
    #[error(
        "no spec available for '{project}': server unreachable and no usable fallback ({detail})"
    )]
    Unavailable { project: String, detail: String },
    #[error("failed to parse OpenAPI spec: {0}")]
    Parse(String),
    #[error("unknown endpoint '{name}'{}", suggest(.suggestions))]
    UnknownEndpoint {
        name: String,
        suggestions: Vec<String>,
    },
    #[error("unknown tag '{name}'{}", suggest(.available))]
    UnknownTag {
        name: String,
        available: Vec<String>,
    },
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("credential unavailable: {0}")]
    Credential(String),
    #[error("login request to {url} failed: {message}")]
    LoginFailed { url: String, message: String },
    #[error("token not found in login response at JSON pointer '{0}'")]
    TokenNotFound(String),
    #[error("OAuth2 flow failed: {0}")]
    OAuth(String),
    #[error("token store error: {0}")]
    Store(String),
    #[error("interactive authentication required: {0}")]
    InteractionRequired(String),
}

#[derive(Debug, Error)]
pub enum RequestError {
    #[error("missing required path parameter '{0}'")]
    MissingPathParam(String),
    #[error("invalid request body: {0}")]
    InvalidBody(String),
    #[error("invalid header '{0}'")]
    InvalidHeader(String),
    #[error("request to {url} failed: {message}")]
    Network { url: String, message: String },
    #[error("request timed out after {0}s")]
    Timeout(u64),
}

/// Umbrella error: everything user-facing funnels through this so the JSON
/// envelope, MCP errors, and exit codes stay consistent.
#[derive(Debug, Error)]
pub enum HitError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Spec(#[from] SpecError),
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Request(#[from] RequestError),
    #[error("{0}")]
    Other(String),
}

impl HitError {
    /// Stable machine-readable error kind, used in the JSON envelope and MCP errors.
    pub fn kind(&self) -> &'static str {
        match self {
            HitError::Config(ConfigError::UnknownProject { .. }) => "unknown_project",
            HitError::Config(_) => "config_error",
            HitError::Spec(SpecError::UnknownEndpoint { .. }) => "unknown_endpoint",
            HitError::Spec(SpecError::UnknownTag { .. }) => "unknown_tag",
            HitError::Spec(SpecError::Unavailable { .. }) => "spec_unavailable",
            HitError::Spec(_) => "spec_error",
            HitError::Auth(AuthError::InteractionRequired(_)) => "auth_interaction_required",
            HitError::Auth(_) => "auth_error",
            HitError::Request(RequestError::Network { .. }) => "network_error",
            HitError::Request(RequestError::Timeout(_)) => "timeout",
            HitError::Request(_) => "request_error",
            HitError::Other(_) => "error",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            HitError::Config(_) => exit_code::USAGE,
            HitError::Spec(_) => exit_code::SPEC,
            HitError::Auth(_) => exit_code::AUTH,
            HitError::Request(RequestError::Network { .. } | RequestError::Timeout(_)) => {
                exit_code::NETWORK
            }
            HitError::Request(_) => exit_code::USAGE,
            HitError::Other(_) => exit_code::USAGE,
        }
    }
}

fn suggest(names: &[String]) -> String {
    if names.is_empty() {
        String::new()
    } else {
        format!(" (did you mean one of: {}?)", names.join(", "))
    }
}
