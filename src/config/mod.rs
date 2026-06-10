//! Loading, validating, and saving `projects.toml`, plus XDG path resolution.

mod schema;

pub use schema::*;

use std::path::{Path, PathBuf};

use crate::error::ConfigError;

/// Resolved filesystem locations for config, cache, and data.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_file: PathBuf,
    pub spec_cache_dir: PathBuf,
    pub token_dir: PathBuf,
    pub log_dir: PathBuf,
}

impl Paths {
    /// Standard XDG locations, or everything rooted next to an explicit config file.
    pub fn resolve(config_override: Option<&Path>) -> Result<Self, ConfigError> {
        if let Some(file) = config_override {
            let root = file.parent().unwrap_or(Path::new(".")).to_path_buf();
            return Ok(Self {
                config_file: file.to_path_buf(),
                spec_cache_dir: root.join("cache/specs"),
                token_dir: root.join("tokens"),
                log_dir: root.join("logs"),
            });
        }
        let dirs = directories::ProjectDirs::from("", "", "hitpoint").ok_or_else(|| {
            ConfigError::Invalid {
                field: "paths".into(),
                message: "could not determine a home directory".into(),
            }
        })?;
        Ok(Self {
            config_file: dirs.config_dir().join("projects.toml"),
            spec_cache_dir: dirs.cache_dir().join("specs"),
            token_dir: dirs.data_dir().join("tokens"),
            log_dir: dirs.data_dir().join("logs"),
        })
    }
}

/// Load and validate the config. A missing file is an empty config, not an error,
/// so `hit projects add` works on first run.
pub fn load(paths: &Paths) -> Result<ProjectsConfig, ConfigError> {
    let raw = match std::fs::read_to_string(&paths.config_file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ProjectsConfig::default());
        }
        Err(e) => {
            return Err(ConfigError::Io {
                path: paths.config_file.display().to_string(),
                source: e,
            });
        }
    };
    let config: ProjectsConfig = toml::from_str(&raw)
        .map_err(|e| ConfigError::Parse(format!("{}: {e}", paths.config_file.display())))?;
    validate(&config)?;
    Ok(config)
}

/// Validation beyond what serde enforces structurally.
pub fn validate(config: &ProjectsConfig) -> Result<(), ConfigError> {
    for (name, project) in &config.projects {
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_alphanumeric() || "-_".contains(c))
        {
            return Err(ConfigError::Invalid {
                field: format!("projects.{name}"),
                message: "project names must be alphanumeric plus '-' or '_'".into(),
            });
        }
        if !matches!(project.base_url.scheme(), "http" | "https") {
            return Err(ConfigError::Invalid {
                field: format!("projects.{name}.base_url"),
                message: format!("unsupported scheme '{}'", project.base_url.scheme()),
            });
        }
        if let Some(spec_file) = &project.spec_file
            && !spec_file.exists()
        {
            // Warn, don't fail: the fallback only matters when the server is down.
            tracing::warn!(
                project = name,
                spec_file = %spec_file.display(),
                "configured spec_file does not exist"
            );
        }
        if let Some(AuthConfig::JwtLogin(jwt)) = &project.auth {
            if !jwt.login_path.starts_with('/') {
                return Err(ConfigError::Invalid {
                    field: format!("projects.{name}.auth.login_path"),
                    message: "login_path must start with '/'".into(),
                });
            }
            if !jwt.token_json_pointer.starts_with('/') {
                return Err(ConfigError::Invalid {
                    field: format!("projects.{name}.auth.token_json_pointer"),
                    message: "JSON pointers must start with '/'".into(),
                });
            }
        }
    }
    Ok(())
}

/// Atomically persist the config (temp file + rename).
pub fn save(paths: &Paths, config: &ProjectsConfig) -> Result<(), ConfigError> {
    validate(config)?;
    let serialized = toml::to_string_pretty(config)
        .map_err(|e| ConfigError::Parse(format!("serialize: {e}")))?;
    let dir = paths
        .config_file
        .parent()
        .ok_or_else(|| ConfigError::Invalid {
            field: "config path".into(),
            message: "config file has no parent directory".into(),
        })?;
    std::fs::create_dir_all(dir).map_err(|e| ConfigError::Io {
        path: dir.display().to_string(),
        source: e,
    })?;
    let tmp = paths.config_file.with_extension("toml.tmp");
    std::fs::write(&tmp, serialized).map_err(|e| ConfigError::Io {
        path: tmp.display().to_string(),
        source: e,
    })?;
    std::fs::rename(&tmp, &paths.config_file).map_err(|e| ConfigError::Io {
        path: paths.config_file.display().to_string(),
        source: e,
    })
}

/// Fetch one project or fail with suggestions.
pub fn project<'a>(
    config: &'a ProjectsConfig,
    name: &str,
) -> Result<&'a ProjectConfig, ConfigError> {
    config
        .projects
        .get(name)
        .ok_or_else(|| ConfigError::UnknownProject {
            name: name.to_string(),
            available: config.projects.keys().cloned().collect(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_example() {
        let toml_src = r#"
            [settings]
            spec_cache_ttl_secs = 60

            [projects.billing]
            base_url = "http://localhost:8000"
            default_headers = { "X-Tenant" = "dev" }

            [projects.billing.auth]
            type = "jwt_login"
            login_path = "/auth/login"
            username = { env = "BILLING_USER" }
            password = { prompt = true }

            [projects.crm]
            base_url = "https://crm.example.com"

            [projects.crm.auth]
            type = "oauth2_pkce"
            auth_url = "https://idp.example.com/authorize"
            token_url = "https://idp.example.com/oauth/token"
            client_id = "hitpoint"
            scopes = ["openid"]
        "#;
        let config: ProjectsConfig = toml::from_str(toml_src).unwrap();
        validate(&config).unwrap();
        assert_eq!(config.settings.spec_cache_ttl_secs, 60);
        assert_eq!(config.settings.timeout_secs, 30); // default survives partial [settings]
        let billing = &config.projects["billing"];
        match billing.auth.as_ref().unwrap() {
            AuthConfig::JwtLogin(jwt) => {
                assert!(matches!(jwt.username, CredentialRef::Env { .. }));
                assert!(matches!(jwt.password, CredentialRef::Prompt { .. }));
                assert_eq!(jwt.token_json_pointer, "/access_token");
            }
            other => panic!("expected jwt_login, got {}", other.type_name()),
        }
        assert!(matches!(
            config.projects["crm"].auth,
            Some(AuthConfig::Oauth2Pkce(_))
        ));
    }

    #[test]
    fn rejects_bad_login_path() {
        let toml_src = r#"
            [projects.x]
            base_url = "http://localhost:1"
            [projects.x.auth]
            type = "jwt_login"
            login_path = "auth/login"
            username = { value = "u" }
            password = { value = "p" }
        "#;
        let config: ProjectsConfig = toml::from_str(toml_src).unwrap();
        assert!(validate(&config).is_err());
    }

    #[test]
    fn unknown_project_lists_alternatives() {
        let config: ProjectsConfig =
            toml::from_str("[projects.alpha]\nbase_url = \"http://x\"").unwrap();
        let err = project(&config, "beta").unwrap_err();
        assert!(err.to_string().contains("alpha"));
    }
}
