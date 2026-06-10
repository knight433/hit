//! Request execution: build the URL from the endpoint template, attach auth,
//! send, and (once) retry on 401 after invalidating cached credentials.

mod response;

pub use response::ApiResponse;

use std::collections::BTreeMap;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

/// Encode everything except RFC 3986 unreserved characters.
const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');
use serde_json::Value;

use crate::auth::AuthManager;
use crate::config::ProjectConfig;
use crate::error::{HitError, RequestError};
use crate::model::Endpoint;

/// User-supplied request inputs, the same shape for CLI, MCP, and TUI.
#[derive(Debug, Clone, Default)]
pub struct RequestArgs {
    pub path_params: BTreeMap<String, String>,
    pub query_params: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Option<Value>,
    pub no_auth: bool,
}

pub struct RequestExecutor<'a> {
    pub client: &'a reqwest::Client,
    pub project: &'a ProjectConfig,
    pub auth: Option<&'a AuthManager>,
}

impl RequestExecutor<'_> {
    pub async fn execute(
        &self,
        endpoint: &Endpoint,
        args: &RequestArgs,
    ) -> Result<ApiResponse, HitError> {
        let url = self.build_url(endpoint, args)?;
        let use_auth = !args.no_auth && self.auth.is_some();

        let mut bearer = match (use_auth, self.auth) {
            (true, Some(auth)) => auth.bearer().await.map(Some)?,
            _ => None,
        };

        let started = std::time::Instant::now();
        let mut response = self.send(endpoint, args, &url, bearer.as_deref()).await?;

        // One reactive retry: cached token may have just expired.
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && let (true, Some(auth)) = (use_auth, self.auth)
        {
            tracing::info!(url, "got 401; invalidating cached token and retrying once");
            auth.invalidate().await;
            bearer = Some(auth.bearer().await?);
            response = self.send(endpoint, args, &url, bearer.as_deref()).await?;
        }

        ApiResponse::from_reqwest(endpoint.method.clone(), url, response, started.elapsed()).await
    }

    fn build_url(&self, endpoint: &Endpoint, args: &RequestArgs) -> Result<String, RequestError> {
        let path = fill_path(&endpoint.path, &args.path_params)?;
        Ok(format!(
            "{}{}",
            self.project.base_url.as_str().trim_end_matches('/'),
            path
        ))
    }

    async fn send(
        &self,
        endpoint: &Endpoint,
        args: &RequestArgs,
        url: &str,
        bearer: Option<&str>,
    ) -> Result<reqwest::Response, HitError> {
        let method: reqwest::Method = endpoint
            .method
            .parse()
            .map_err(|_| RequestError::InvalidHeader(endpoint.method.clone()))?;
        let mut request = self.client.request(method, url);

        for (name, value) in &self.project.default_headers {
            request = request.header(name, value);
        }
        for (name, value) in &args.headers {
            request = request.header(name, value);
        }
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        if !args.query_params.is_empty() {
            request = request.query(&args.query_params);
        }
        if let Some(body) = &args.body {
            request = attach_body(request, endpoint, body)?;
        }

        request.send().await.map_err(|e| {
            if e.is_timeout() {
                HitError::Request(RequestError::Timeout(0))
            } else {
                HitError::Request(RequestError::Network {
                    url: url.to_string(),
                    message: e.to_string(),
                })
            }
        })
    }
}

/// Attach the body using the endpoint's declared content type.
fn attach_body(
    request: reqwest::RequestBuilder,
    endpoint: &Endpoint,
    body: &Value,
) -> Result<reqwest::RequestBuilder, RequestError> {
    let content_type = endpoint
        .body
        .as_ref()
        .map(|b| b.content_type.as_str())
        .unwrap_or("application/json");

    if content_type == "application/x-www-form-urlencoded" {
        let map = body.as_object().ok_or_else(|| {
            RequestError::InvalidBody("form-encoded endpoints need a JSON object body".into())
        })?;
        let form: Vec<(String, String)> = map
            .iter()
            .filter(|(_, v)| !v.is_null())
            .map(|(k, v)| (k.clone(), scalar_to_string(v)))
            .collect();
        return Ok(request.form(&form));
    }
    if content_type.starts_with("multipart/") {
        return Err(RequestError::InvalidBody(
            "multipart/form-data bodies are not supported yet".into(),
        ));
    }
    Ok(request.json(body))
}

fn scalar_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Substitute `{param}` placeholders, percent-encoding values.
fn fill_path(template: &str, params: &BTreeMap<String, String>) -> Result<String, RequestError> {
    let mut result = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        result.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after
            .find('}')
            .ok_or_else(|| RequestError::InvalidBody(format!("malformed path '{template}'")))?;
        let name = &after[..end];
        let value = params
            .get(name)
            .ok_or_else(|| RequestError::MissingPathParam(name.to_string()))?;
        result.extend(utf8_percent_encode(value, PATH_SEGMENT));
        rest = &after[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_and_encodes_path_params() {
        let mut params = BTreeMap::new();
        params.insert("user_id".to_string(), "a b/c".to_string());
        assert_eq!(
            fill_path("/users/{user_id}/posts", &params).unwrap(),
            "/users/a%20b%2Fc/posts"
        );
    }

    #[test]
    fn missing_path_param_errors() {
        let err = fill_path("/users/{user_id}", &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, RequestError::MissingPathParam(name) if name == "user_id"));
    }
}
