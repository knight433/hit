//! The machine envelope and human/JSON printing for CLI commands.

use serde_json::{Value, json};

use crate::error::HitError;

pub struct CommandOutput {
    pub data: Value,
    pub human: String,
    pub exit_code: i32,
}

impl CommandOutput {
    pub fn ok(data: Value, human: impl Into<String>) -> Self {
        Self {
            data,
            human: human.into(),
            exit_code: crate::error::exit_code::OK,
        }
    }

    pub fn with_exit(mut self, code: i32) -> Self {
        self.exit_code = code;
        self
    }
}

/// Print the outcome (envelope on stdout in JSON mode, human text otherwise;
/// errors go to stderr in human mode) and return the process exit code.
pub fn print_result(json_mode: bool, result: Result<CommandOutput, HitError>) -> i32 {
    match result {
        Ok(output) => {
            if json_mode {
                let envelope = json!({"ok": true, "data": output.data, "error": null});
                println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
            } else if !output.human.is_empty() {
                println!("{}", output.human);
            }
            output.exit_code
        }
        Err(error) => {
            if json_mode {
                let envelope = json!({
                    "ok": false,
                    "data": null,
                    "error": {"kind": error.kind(), "message": error.to_string()},
                });
                println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
            } else {
                eprintln!("error: {error}");
            }
            error.exit_code()
        }
    }
}
