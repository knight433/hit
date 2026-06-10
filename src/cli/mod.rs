//! clap definitions and headless command handlers.

pub mod output;

use std::collections::BTreeMap;
use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, CommandFactory, Parser, Subcommand};
use serde_json::{Value, json};
use url::Url;

use crate::auth::{AuthManager, CliInteractor};
use crate::config::{self, ProjectConfig};
use crate::error::{ConfigError, HitError, RequestError, exit_code};
use crate::http::{RequestArgs, RequestExecutor};
use crate::model::build_template;
use crate::spec::adapter::adapter_for;
use crate::{AppServices, spec};
use output::CommandOutput;

#[derive(Parser, Debug)]
#[command(
    name = "hit",
    version,
    about = "Browse and hit your projects' APIs — interactively or from scripts/agents",
    long_about = "hitpoint: a terminal API tester for FastAPI backends.\n\
                  Run with no arguments for the interactive TUI. Every subcommand supports\n\
                  --json for machine-readable output (automatic when stdout is not a TTY)."
)]
pub struct Cli {
    /// Path to projects.toml (defaults to ~/.config/hitpoint/projects.toml)
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Emit a JSON envelope {ok, data, error} on stdout
    #[arg(long, global = true)]
    pub json: bool,

    /// Increase log verbosity (-v info, -vv debug)
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Bypass the spec cache and re-fetch openapi.json
    #[arg(long, global = true)]
    pub no_cache: bool,

    /// Request timeout in seconds (overrides settings.timeout_secs)
    #[arg(long, global = true, value_name = "SECS")]
    pub timeout: Option<u64>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage registered projects
    Projects {
        #[command(subcommand)]
        cmd: ProjectsCmd,
    },
    /// List a project's OpenAPI tags
    Tags { project: String },
    /// List endpoints, optionally filtered by tag or search string
    Endpoints {
        project: String,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        search: Option<String>,
    },
    /// Print a fill-in-the-blanks request template for an endpoint
    Template {
        project: String,
        /// operation_id or "METHOD /path"
        endpoint: String,
    },
    /// Execute a request against an endpoint
    Run(RunArgs),
    /// Authenticate now and cache the token
    Login { project: String },
    /// Clear the cached token
    Logout { project: String },
    /// Spec cache operations
    Spec {
        #[command(subcommand)]
        cmd: SpecCmd,
    },
    /// Config file operations
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Run as an MCP server on stdio (for AI agents)
    Mcp,
    /// Open the interactive TUI (same as running with no arguments)
    Tui { project: Option<String> },
    /// Generate shell completions
    Completions { shell: clap_complete::Shell },
}

#[derive(Subcommand, Debug)]
pub enum ProjectsCmd {
    /// List registered projects
    List,
    /// Register a project
    Add {
        name: String,
        #[arg(long)]
        base_url: Url,
        /// On-disk openapi.json fallback used when the server is down
        #[arg(long)]
        spec_file: Option<PathBuf>,
        /// Default header sent with every request, as "Key: Value" (repeatable)
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
    },
    /// Unregister a project (tokens and cache are also removed)
    Remove { name: String },
}

#[derive(Subcommand, Debug)]
pub enum SpecCmd {
    /// Re-fetch openapi.json and refresh the cache
    Refresh { project: String },
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Validate projects.toml
    Check,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    pub project: String,
    /// operation_id or "METHOD /path"
    pub endpoint: String,
    /// JSON body: inline string, @file.json, or '-' for stdin
    #[arg(long)]
    pub body: Option<String>,
    /// Path parameter, as name=value (repeatable)
    #[arg(short = 'p', long = "path-param", value_name = "NAME=VALUE")]
    pub path_params: Vec<String>,
    /// Query parameter, as name=value (repeatable)
    #[arg(short = 'q', long = "query", value_name = "NAME=VALUE")]
    pub query: Vec<String>,
    /// Extra header, as "Key: Value" (repeatable)
    #[arg(short = 'H', long = "header", value_name = "KEY: VALUE")]
    pub headers: Vec<String>,
    /// Skip authentication for this request
    #[arg(long)]
    pub no_auth: bool,
    /// Exit 0 even when the response is an HTTP error
    #[arg(long)]
    pub allow_error: bool,
}

/// Whether output should be the JSON envelope.
pub fn json_mode(cli: &Cli) -> bool {
    cli.json || !std::io::stdout().is_terminal()
}

/// Run a headless subcommand; returns the process exit code.
pub async fn run(cli: Cli, services: AppServices) -> i32 {
    let json = json_mode(&cli);
    let Some(command) = cli.command else {
        unreachable!("no-subcommand launches the TUI from main");
    };
    let result = dispatch(command, &cli.config, cli.no_cache, services).await;
    output::print_result(json, result)
}

async fn dispatch(
    command: Command,
    config_override: &Option<PathBuf>,
    no_cache: bool,
    mut services: AppServices,
) -> Result<CommandOutput, HitError> {
    match command {
        Command::Projects { cmd } => projects_cmd(cmd, &mut services).await,
        Command::Tags { project } => tags_cmd(&project, no_cache, &services).await,
        Command::Endpoints {
            project,
            tag,
            search,
        } => {
            endpoints_cmd(
                &project,
                tag.as_deref(),
                search.as_deref(),
                no_cache,
                &services,
            )
            .await
        }
        Command::Template { project, endpoint } => {
            template_cmd(&project, &endpoint, no_cache, &services).await
        }
        Command::Run(args) => run_cmd(args, no_cache, &services).await,
        Command::Login { project } => login_cmd(&project, &services).await,
        Command::Logout { project } => logout_cmd(&project, &services),
        Command::Spec { cmd } => match cmd {
            SpecCmd::Refresh { project } => refresh_cmd(&project, &services).await,
        },
        Command::Config { cmd } => match cmd {
            ConfigCmd::Check => config_check_cmd(config_override, &services),
        },
        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "hit", &mut std::io::stdout());
            Ok(CommandOutput::ok(Value::Null, ""))
        }
        Command::Mcp | Command::Tui { .. } => {
            unreachable!("mcp/tui are dispatched from main")
        }
    }
}

async fn projects_cmd(
    cmd: ProjectsCmd,
    services: &mut AppServices,
) -> Result<CommandOutput, HitError> {
    match cmd {
        ProjectsCmd::List => {
            let rows: Vec<Value> = services
                .config
                .projects
                .iter()
                .map(|(name, p)| {
                    json!({
                        "name": name,
                        "base_url": p.base_url.as_str(),
                        "auth_type": p.auth.as_ref().map(|a| a.type_name()),
                        "spec_file": p.spec_file.as_ref().map(|f| f.display().to_string()),
                    })
                })
                .collect();
            let human = if rows.is_empty() {
                "no projects registered — add one with: hit projects add <name> --base-url <url>"
                    .to_string()
            } else {
                services
                    .config
                    .projects
                    .iter()
                    .map(|(name, p)| {
                        format!(
                            "{name:<20} {}  [auth: {}]",
                            p.base_url,
                            p.auth.as_ref().map_or("none", |a| a.type_name())
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            Ok(CommandOutput::ok(json!(rows), human))
        }
        ProjectsCmd::Add {
            name,
            base_url,
            spec_file,
            headers,
        } => {
            if services.config.projects.contains_key(&name) {
                return Err(ConfigError::DuplicateProject(name).into());
            }
            let mut default_headers = BTreeMap::new();
            for header in &headers {
                let (key, value) = parse_header(header)?;
                default_headers.insert(key, value);
            }
            services.config.projects.insert(
                name.clone(),
                ProjectConfig {
                    base_url,
                    spec_file,
                    default_headers,
                    auth: None,
                    framework: Default::default(),
                },
            );
            config::save(&services.paths, &services.config)?;
            Ok(CommandOutput::ok(
                json!({"added": name}),
                format!(
                    "added project '{name}'. Configure auth by editing {}",
                    services.paths.config_file.display()
                ),
            ))
        }
        ProjectsCmd::Remove { name } => {
            if services.config.projects.remove(&name).is_none() {
                return Err(ConfigError::UnknownProject {
                    name,
                    available: services.config.projects.keys().cloned().collect(),
                }
                .into());
            }
            config::save(&services.paths, &services.config)?;
            // Best-effort cleanup of cached state.
            let _ =
                std::fs::remove_file(services.paths.spec_cache_dir.join(format!("{name}.json")));
            let _ = std::fs::remove_file(services.paths.token_dir.join(format!("{name}.json")));
            Ok(CommandOutput::ok(
                json!({"removed": name}),
                format!("removed project '{name}'"),
            ))
        }
    }
}

async fn load_spec(
    project_name: &str,
    no_cache: bool,
    services: &AppServices,
) -> Result<spec::LoadedSpec, HitError> {
    let project = config::project(&services.config, project_name)?;
    spec::load(
        &services.client,
        project_name,
        project,
        services.settings(),
        &services.paths.spec_cache_dir,
        no_cache,
    )
    .await
    .map_err(Into::into)
}

async fn tags_cmd(
    project: &str,
    no_cache: bool,
    services: &AppServices,
) -> Result<CommandOutput, HitError> {
    let loaded = load_spec(project, no_cache, services).await?;
    let rows: Vec<Value> = loaded
        .spec
        .tags
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "endpoint_count": t.endpoint_ids.len(),
            })
        })
        .collect();
    let human = loaded
        .spec
        .tags
        .iter()
        .map(|t| {
            format!(
                "{:<24} {:>3} endpoints  {}",
                t.name,
                t.endpoint_ids.len(),
                t.description.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(CommandOutput::ok(json!(rows), human))
}

async fn endpoints_cmd(
    project: &str,
    tag: Option<&str>,
    search: Option<&str>,
    no_cache: bool,
    services: &AppServices,
) -> Result<CommandOutput, HitError> {
    let loaded = load_spec(project, no_cache, services).await?;
    let spec = &loaded.spec;

    let in_tag: Option<Vec<&str>> = match tag {
        Some(tag_name) => Some(
            spec.tag(tag_name)?
                .endpoint_ids
                .iter()
                .map(String::as_str)
                .collect(),
        ),
        None => None,
    };

    let needle = search.map(str::to_ascii_lowercase);
    let endpoints: Vec<_> = spec
        .endpoints
        .iter()
        .filter(|e| {
            in_tag
                .as_ref()
                .map(|ids| ids.contains(&e.id.as_str()))
                .unwrap_or(true)
        })
        .filter(|e| {
            needle.as_ref().is_none_or(|n| {
                e.id.to_ascii_lowercase().contains(n)
                    || e.path.to_ascii_lowercase().contains(n)
                    || e.summary
                        .as_deref()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains(n)
            })
        })
        .collect();

    let rows: Vec<Value> = endpoints
        .iter()
        .map(|e| {
            json!({
                "id": e.id,
                "method": e.method,
                "path": e.path,
                "summary": e.summary,
                "tags": e.tags,
                "has_body": e.body.is_some(),
                "auth_required": e.auth_required,
                "deprecated": e.deprecated,
            })
        })
        .collect();
    let human = endpoints
        .iter()
        .map(|e| {
            format!(
                "{:<7} {:<40} {}{}",
                e.method,
                e.path,
                e.summary.as_deref().unwrap_or(&e.id),
                if e.deprecated { "  [deprecated]" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(CommandOutput::ok(json!(rows), human))
}

async fn template_cmd(
    project: &str,
    endpoint: &str,
    no_cache: bool,
    services: &AppServices,
) -> Result<CommandOutput, HitError> {
    let loaded = load_spec(project, no_cache, services).await?;
    let endpoint = loaded.spec.find_endpoint(endpoint)?;
    let template = build_template(endpoint);
    let data = serde_json::to_value(&template).map_err(|e| HitError::Other(e.to_string()))?;
    let human = serde_json::to_string_pretty(&data).unwrap_or_default();
    Ok(CommandOutput::ok(data, human))
}

async fn run_cmd(
    args: RunArgs,
    no_cache: bool,
    services: &AppServices,
) -> Result<CommandOutput, HitError> {
    let project = config::project(&services.config, &args.project)?;
    let loaded = load_spec(&args.project, no_cache, services).await?;
    let endpoint = loaded.spec.find_endpoint(&args.endpoint)?;

    let mut request_args = RequestArgs {
        no_auth: args.no_auth,
        body: args.body.as_deref().map(parse_body).transpose()?,
        ..Default::default()
    };
    for kv in &args.path_params {
        let (k, v) = parse_kv(kv)?;
        request_args.path_params.insert(k, v);
    }
    for kv in &args.query {
        request_args.query_params.push(parse_kv(kv)?);
    }
    for header in &args.headers {
        request_args.headers.push(parse_header(header)?);
    }

    let auth = AuthManager::for_project(
        &args.project,
        project,
        services.settings(),
        &services.paths,
        services.client.clone(),
        Arc::new(CliInteractor),
        false,
    )?;

    let executor = RequestExecutor {
        client: &services.client,
        project,
        auth: auth.as_ref(),
    };
    let response = executor.execute(endpoint, &request_args).await?;

    let mut human = format!(
        "{} {} -> {} ({} ms)",
        response.method, response.url, response.status, response.latency_ms
    );
    if !response.is_success()
        && let Some(lines) =
            adapter_for(project.framework).render_error_lines(response.status, &response.body)
    {
        for line in &lines {
            human.push_str(&format!("\n  ! {line}"));
        }
    }
    let body_pretty = if response.body_is_json {
        serde_json::to_string_pretty(&response.body).unwrap_or_default()
    } else {
        response.body.as_str().unwrap_or("").to_string()
    };
    if !body_pretty.is_empty() {
        human.push_str("\n\n");
        human.push_str(&body_pretty);
    }

    let exit = if args.allow_error {
        exit_code::OK
    } else {
        response.exit_code()
    };
    let data = serde_json::to_value(&response).map_err(|e| HitError::Other(e.to_string()))?;
    Ok(CommandOutput::ok(data, human).with_exit(exit))
}

async fn login_cmd(project_name: &str, services: &AppServices) -> Result<CommandOutput, HitError> {
    let project = config::project(&services.config, project_name)?;
    let auth = AuthManager::for_project(
        project_name,
        project,
        services.settings(),
        &services.paths,
        services.client.clone(),
        Arc::new(CliInteractor),
        false,
    )?
    .ok_or_else(|| HitError::Other(format!("project '{project_name}' has no auth configured")))?;

    auth.invalidate().await; // force a fresh login
    auth.bearer().await?;
    let expiry = auth.cached_expiry();
    let human = match expiry {
        Some(exp) => {
            let remaining = exp.saturating_sub(crate::auth::token_store::now_unix());
            format!("logged in to '{project_name}' (token expires in {remaining}s)")
        }
        None => format!("logged in to '{project_name}' (token has no visible expiry)"),
    };
    Ok(CommandOutput::ok(
        json!({"project": project_name, "expires_at_unix": expiry}),
        human,
    ))
}

fn logout_cmd(project_name: &str, services: &AppServices) -> Result<CommandOutput, HitError> {
    config::project(&services.config, project_name)?;
    let store = crate::auth::new_token_store(
        services.settings().token_store,
        services.paths.token_dir.clone(),
    )?;
    store.clear(project_name)?;
    Ok(CommandOutput::ok(
        json!({"project": project_name}),
        format!("cleared cached token for '{project_name}'"),
    ))
}

async fn refresh_cmd(
    project_name: &str,
    services: &AppServices,
) -> Result<CommandOutput, HitError> {
    let project = config::project(&services.config, project_name)?;
    let loaded = spec::refresh(
        &services.client,
        project_name,
        project,
        &services.paths.spec_cache_dir,
    )
    .await?;
    let data = json!({
        "title": loaded.spec.title,
        "version": loaded.spec.version,
        "openapi_version": loaded.spec.openapi_version,
        "tags": loaded.spec.tags.len(),
        "endpoints": loaded.spec.endpoints.len(),
    });
    let human = format!(
        "refreshed '{project_name}': {} v{} — {} endpoints across {} tags",
        loaded.spec.title,
        loaded.spec.version,
        loaded.spec.endpoints.len(),
        loaded.spec.tags.len()
    );
    Ok(CommandOutput::ok(data, human))
}

fn config_check_cmd(
    config_override: &Option<PathBuf>,
    services: &AppServices,
) -> Result<CommandOutput, HitError> {
    // Config was already loaded+validated at startup; re-validate explicitly
    // so the command works as a standalone health check.
    config::validate(&services.config)?;
    let path = config_override
        .clone()
        .unwrap_or_else(|| services.paths.config_file.clone());
    let human = format!(
        "{} OK — {} project(s)",
        path.display(),
        services.config.projects.len()
    );
    Ok(CommandOutput::ok(
        json!({"projects": services.config.projects.len()}),
        human,
    ))
}

fn parse_kv(input: &str) -> Result<(String, String), HitError> {
    input
        .split_once('=')
        .map(|(k, v)| (k.trim().to_string(), v.to_string()))
        .ok_or_else(|| {
            HitError::Request(RequestError::InvalidBody(format!(
                "expected name=value, got '{input}'"
            )))
        })
}

fn parse_header(input: &str) -> Result<(String, String), HitError> {
    input
        .split_once(':')
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .ok_or_else(|| HitError::Request(RequestError::InvalidHeader(input.to_string())))
}

/// Parse --body: inline JSON, @file, or '-' for stdin.
fn parse_body(input: &str) -> Result<Value, HitError> {
    let raw = if input == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| HitError::Request(RequestError::InvalidBody(format!("stdin: {e}"))))?;
        buf
    } else if let Some(path) = input.strip_prefix('@') {
        std::fs::read_to_string(path)
            .map_err(|e| HitError::Request(RequestError::InvalidBody(format!("{path}: {e}"))))?
    } else {
        input.to_string()
    };
    serde_json::from_str(&raw)
        .map_err(|e| HitError::Request(RequestError::InvalidBody(e.to_string())))
}
