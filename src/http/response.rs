//! The response shape shared by CLI output, MCP results, and the TUI viewer.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::error::HitError;

#[derive(Debug, Clone, Serialize)]
pub struct ApiResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Parsed JSON when the body is JSON, otherwise a JSON string of the raw text.
    pub body: Value,
    pub body_is_json: bool,
    pub latency_ms: u64,
    pub url: String,
    pub method: String,
}

impl ApiResponse {
    pub async fn from_reqwest(
        method: String,
        url: String,
        response: reqwest::Response,
        latency: Duration,
    ) -> Result<Self, HitError> {
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.to_string(),
                    value.to_str().unwrap_or("<binary>").to_string(),
                )
            })
            .collect();
        let text = response.text().await.map_err(|e| {
            HitError::Request(crate::error::RequestError::Network {
                url: url.clone(),
                message: format!("reading response body: {e}"),
            })
        })?;
        let (body, body_is_json) = match serde_json::from_str::<Value>(&text) {
            Ok(parsed) => (parsed, true),
            Err(_) => (Value::String(text), false),
        };
        Ok(Self {
            status,
            headers,
            body,
            body_is_json,
            latency_ms: latency.as_millis() as u64,
            url,
            method,
        })
    }

    pub fn is_success(&self) -> bool {
        self.status < 400
    }

    /// Exit code contribution: 5 for 4xx, 6 for 5xx, 0 otherwise.
    pub fn exit_code(&self) -> i32 {
        match self.status {
            400..=499 => crate::error::exit_code::HTTP_4XX,
            500..=599 => crate::error::exit_code::HTTP_5XX,
            _ => crate::error::exit_code::OK,
        }
    }
}
