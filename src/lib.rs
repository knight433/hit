//! hitpoint — terminal API tester for FastAPI backends.
//!
//! One core, three frontends: an interactive TUI, headless CLI subcommands,
//! and an MCP server. The frontends share `AppServices` and never call each
//! other.

pub mod auth;
pub mod cli;
pub mod config;
pub mod error;
pub mod http;
pub mod mcp;
pub mod model;
pub mod spec;
pub mod tui;

use std::time::Duration;

use crate::config::{Paths, ProjectsConfig, Settings};

/// Shared services handed to every frontend.
pub struct AppServices {
    pub paths: Paths,
    pub config: ProjectsConfig,
    pub client: reqwest::Client,
}

impl AppServices {
    pub fn new(paths: Paths, config: ProjectsConfig, timeout_override: Option<u64>) -> Self {
        let timeout = timeout_override.unwrap_or(config.settings.timeout_secs);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout))
            .user_agent(concat!("hitpoint/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client construction cannot fail with static config");
        Self {
            paths,
            config,
            client,
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.config.settings
    }
}
