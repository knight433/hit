//! Entry point: mode dispatch (TUI / CLI / MCP) and per-mode tracing setup.
//! The tracing writer is chosen before anything else runs — the TUI and MCP
//! modes must never write logs to stdout/stderr (alt screen / protocol).

use clap::Parser;
use tracing_subscriber::EnvFilter;

use hitpoint::cli::{Cli, Command};
use hitpoint::config::Paths;
use hitpoint::{AppServices, cli, config, mcp, tui};

#[tokio::main]
async fn main() {
    let parsed = Cli::parse();

    let paths = match Paths::resolve(parsed.config.as_deref()) {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(hitpoint::error::exit_code::USAGE);
        }
    };

    init_tracing(&parsed, &paths);

    let config = match config::load(&paths) {
        Ok(config) => config,
        Err(e) => {
            let err = hitpoint::error::HitError::from(e);
            if cli::json_mode(&parsed) {
                let envelope = serde_json::json!({
                    "ok": false,
                    "data": null,
                    "error": {"kind": err.kind(), "message": err.to_string()},
                });
                println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
            } else {
                eprintln!("error: {err}");
            }
            std::process::exit(err.exit_code());
        }
    };

    let services = AppServices::new(paths, config, parsed.timeout);

    let exit = match parsed.command {
        None => tui::run(services, None).await,
        Some(Command::Tui { ref project }) => {
            let project = project.clone();
            tui::run(services, project).await
        }
        Some(Command::Mcp) => mcp::serve(services).await,
        Some(_) => cli::run(parsed, services).await,
    };
    std::process::exit(exit);
}

fn init_tracing(parsed: &Cli, paths: &Paths) {
    let level = match parsed.verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("hitpoint={level}")));

    let to_file = matches!(
        parsed.command,
        None | Some(Command::Tui { .. }) | Some(Command::Mcp)
    );
    if to_file {
        if std::fs::create_dir_all(&paths.log_dir).is_ok() {
            let appender = tracing_appender::rolling::daily(&paths.log_dir, "hitpoint.log");
            // Intentionally leak the guard: logging lives for the whole process.
            let (writer, guard) = tracing_appender::non_blocking(appender);
            std::mem::forget(guard);
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(writer)
                .with_ansi(false)
                .init();
        }
        // If the log dir can't be created, stay silent rather than corrupt
        // the TUI screen or the MCP stdio protocol.
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
    }
}
