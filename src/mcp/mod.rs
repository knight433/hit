//! MCP server mode (`hit mcp`): the same capabilities as the headless CLI,
//! exposed as MCP tools over stdio for AI agents.
//!
//! stdout is protocol — logging goes to file only (set up in main).

use std::collections::BTreeMap;
use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{AuthManager, DenyInteractor};
use crate::error::HitError;
use crate::http::{RequestArgs, RequestExecutor};
use crate::model::build_template;
use crate::{AppServices, config, spec};

pub async fn serve(services: AppServices) -> i32 {
    let server = HitpointServer {
        services: Arc::new(services),
    };
    let running = match server.serve(rmcp::transport::stdio()).await {
        Ok(running) => running,
        Err(e) => {
            tracing::error!(error = %e, "failed to start MCP server");
            return 1;
        }
    };
    if let Err(e) = running.waiting().await {
        tracing::error!(error = %e, "MCP server terminated abnormally");
        return 1;
    }
    0
}

struct HitpointServer {
    services: Arc<AppServices>,
}

fn tool_error(error: HitError) -> ErrorData {
    ErrorData::invalid_params(error.to_string(), Some(json!({"kind": error.kind()})))
}

#[derive(Deserialize, JsonSchema)]
struct ProjectParam {
    /// Registered project name (see list_projects).
    project: String,
}

#[derive(Deserialize, JsonSchema)]
struct ListEndpointsParams {
    /// Registered project name (see list_projects).
    project: String,
    /// Restrict to one OpenAPI tag (see list_tags).
    #[serde(default)]
    tag: Option<String>,
    /// Case-insensitive substring match on id, path, or summary.
    #[serde(default)]
    search: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct EndpointParams {
    /// Registered project name (see list_projects).
    project: String,
    /// Endpoint id (operation_id) or "METHOD /path", e.g. "POST /users/".
    endpoint: String,
}

#[derive(Deserialize, JsonSchema)]
struct ExecuteParams {
    /// Registered project name (see list_projects).
    project: String,
    /// Endpoint id (operation_id) or "METHOD /path", e.g. "POST /users/".
    endpoint: String,
    /// JSON request body. Call get_request_template first to learn the shape;
    /// omit optional fields you don't need (listed in optional_paths).
    #[serde(default)]
    body: Option<Value>,
    /// Values for the {placeholders} in the endpoint path.
    #[serde(default)]
    path_params: Option<BTreeMap<String, String>>,
    /// Query-string parameters.
    #[serde(default)]
    query_params: Option<BTreeMap<String, String>>,
    /// Extra request headers.
    #[serde(default)]
    headers: Option<BTreeMap<String, String>>,
    /// Skip authentication for this request.
    #[serde(default)]
    no_auth: bool,
}

#[tool_router]
impl HitpointServer {
    async fn load_spec(&self, project_name: &str) -> Result<spec::LoadedSpec, ErrorData> {
        let project = config::project(&self.services.config, project_name)
            .map_err(|e| tool_error(e.into()))?;
        spec::load(
            &self.services.client,
            project_name,
            project,
            self.services.settings(),
            &self.services.paths.spec_cache_dir,
            false,
        )
        .await
        .map_err(|e| tool_error(e.into()))
    }

    #[tool(
        name = "list_projects",
        description = "List the registered API projects this machine can test. Start here."
    )]
    async fn list_projects(&self) -> CallToolResult {
        let projects: Vec<Value> = self
            .services
            .config
            .projects
            .iter()
            .map(|(name, p)| {
                json!({
                    "name": name,
                    "base_url": p.base_url.as_str(),
                    "auth_type": p.auth.as_ref().map(|a| a.type_name()),
                })
            })
            .collect();
        CallToolResult::structured(json!({"projects": projects}))
    }

    #[tool(
        name = "list_tags",
        description = "List a project's OpenAPI tags (endpoint groups) with endpoint counts."
    )]
    async fn list_tags(
        &self,
        Parameters(params): Parameters<ProjectParam>,
    ) -> Result<CallToolResult, ErrorData> {
        let loaded = self.load_spec(&params.project).await?;
        let tags: Vec<Value> = loaded
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
        Ok(CallToolResult::structured(json!({"tags": tags})))
    }

    #[tool(
        name = "list_endpoints",
        description = "List a project's endpoints (id, method, path, summary), optionally \
                       filtered by tag or search string. Use the id with \
                       get_request_template / execute_request."
    )]
    async fn list_endpoints(
        &self,
        Parameters(params): Parameters<ListEndpointsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let loaded = self.load_spec(&params.project).await?;
        let spec = &loaded.spec;

        let in_tag: Option<Vec<&str>> = match &params.tag {
            Some(tag_name) => Some(
                spec.tag(tag_name)
                    .map_err(|e| tool_error(e.into()))?
                    .endpoint_ids
                    .iter()
                    .map(String::as_str)
                    .collect(),
            ),
            None => None,
        };
        let needle = params.search.as_deref().map(str::to_ascii_lowercase);

        let endpoints: Vec<Value> = spec
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
        Ok(CallToolResult::structured(json!({"endpoints": endpoints})))
    }

    #[tool(
        name = "get_request_template",
        description = "Get a fill-in-the-blanks request template for an endpoint: an example \
                       body with placeholders, the body schema, required path/query/header \
                       params, plus optional_paths (droppable fields) and nullable_paths \
                       (fields accepting null). Call this BEFORE execute_request."
    )]
    async fn get_request_template(
        &self,
        Parameters(params): Parameters<EndpointParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let loaded = self.load_spec(&params.project).await?;
        let endpoint = loaded
            .spec
            .find_endpoint(&params.endpoint)
            .map_err(|e| tool_error(e.into()))?;
        let template = build_template(endpoint);
        serde_json::to_value(&template)
            .map(CallToolResult::structured)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(
        name = "execute_request",
        description = "Execute a request against an endpoint and return {status, headers, body, \
                       latency_ms, url}. Configured project auth (JWT login) is applied \
                       automatically; OAuth projects must be logged in beforehand via \
                       `hit login <project>` in a terminal. A non-2xx HTTP status is a \
                       successful tool call — inspect `status`."
    )]
    async fn execute_request(
        &self,
        Parameters(params): Parameters<ExecuteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let loaded = self.load_spec(&params.project).await?;
        let endpoint = loaded
            .spec
            .find_endpoint(&params.endpoint)
            .map_err(|e| tool_error(e.into()))?
            .clone();
        let project = config::project(&self.services.config, &params.project)
            .map_err(|e| tool_error(e.into()))?;

        let args = RequestArgs {
            path_params: params.path_params.unwrap_or_default(),
            query_params: params
                .query_params
                .unwrap_or_default()
                .into_iter()
                .collect(),
            headers: params.headers.unwrap_or_default().into_iter().collect(),
            body: params.body,
            no_auth: params.no_auth,
        };

        let interactor = Arc::new(DenyInteractor {
            instruction: format!(
                "interactive auth required — run `hit login {}` in a terminal, then retry",
                params.project
            ),
        });
        let auth = AuthManager::for_project(
            &params.project,
            project,
            self.services.settings(),
            &self.services.paths,
            self.services.client.clone(),
            interactor,
            true,
        )
        .map_err(|e| tool_error(e.into()))?;

        // Never launch a browser from MCP mode.
        if let Some(manager) = &auth
            && !manager.supports_headless()
            && manager.cached_expiry().is_none()
            && !args.no_auth
        {
            return Err(tool_error(HitError::Auth(
                crate::error::AuthError::InteractionRequired(format!(
                    "project '{}' uses browser-based OAuth and has no cached token — run \
                     `hit login {}` in a terminal first",
                    params.project, params.project
                )),
            )));
        }

        let executor = RequestExecutor {
            client: &self.services.client,
            project,
            auth: auth.as_ref(),
        };
        let response = executor
            .execute(&endpoint, &args)
            .await
            .map_err(tool_error)?;
        serde_json::to_value(&response)
            .map(CallToolResult::structured)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }
}

#[tool_handler]
impl ServerHandler for HitpointServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.instructions = Some(
            "hitpoint: test the user's registered API backends. Workflow: \
             list_projects -> list_tags / list_endpoints -> get_request_template \
             -> execute_request. Always fetch the template before executing an \
             endpoint with a body; optional_paths in the template lists fields \
             you may omit, nullable_paths lists fields that accept null."
                .into(),
        );
        info
    }
}
