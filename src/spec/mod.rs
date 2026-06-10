//! Spec acquisition (live fetch / disk fallback / cache) and the build step
//! that turns raw OpenAPI into the normalized `ApiSpec` domain model.

pub mod adapter;
pub mod normalize;
pub mod raw;
pub mod resolve;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{ProjectConfig, Settings};
use crate::error::SpecError;
use crate::model::{ApiSpec, BodySpec, Endpoint, Param, ParamLocation, TagGroup};
use raw::{RawParameter, RawSpec};

/// Cached fetch result on disk: the raw document plus when we got it.
#[derive(Serialize, Deserialize)]
struct CacheEntry {
    fetched_at_unix: u64,
    openapi: Value,
}

/// Where a spec document ultimately came from, surfaced in CLI/MCP output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecOrigin {
    Cache,
    Live,
    File,
    StaleCache,
}

pub struct LoadedSpec {
    pub spec: ApiSpec,
    pub origin: SpecOrigin,
    /// The raw document, kept for schema normalization context ($ref targets).
    pub document: Value,
}

/// Load a project's spec: fresh cache -> live fetch -> disk file -> stale cache.
pub async fn load(
    client: &reqwest::Client,
    project_name: &str,
    project: &ProjectConfig,
    settings: &Settings,
    cache_dir: &Path,
    no_cache: bool,
) -> Result<LoadedSpec, SpecError> {
    let cache_path = cache_file(cache_dir, project_name);
    let ttl = Duration::from_secs(settings.spec_cache_ttl_secs);

    if !no_cache
        && let Some(entry) = read_cache(&cache_path)
        && cache_age(&entry).map(|age| age < ttl).unwrap_or(false)
    {
        let spec = build(&entry.openapi)?;
        return Ok(LoadedSpec {
            spec,
            origin: SpecOrigin::Cache,
            document: entry.openapi,
        });
    }

    let url = spec_url(project);
    match fetch_live(client, &url).await {
        Ok(document) => {
            let spec = build(&document)?;
            write_cache(&cache_path, &document);
            Ok(LoadedSpec {
                spec,
                origin: SpecOrigin::Live,
                document,
            })
        }
        Err(fetch_err) => {
            tracing::warn!(url, error = %fetch_err, "live spec fetch failed; trying fallbacks");
            if let Some(spec_file) = &project.spec_file {
                let document = read_spec_file(spec_file)?;
                let spec = build(&document)?;
                return Ok(LoadedSpec {
                    spec,
                    origin: SpecOrigin::File,
                    document,
                });
            }
            if let Some(entry) = read_cache(&cache_path) {
                tracing::warn!("serving stale cached spec");
                let spec = build(&entry.openapi)?;
                return Ok(LoadedSpec {
                    spec,
                    origin: SpecOrigin::StaleCache,
                    document: entry.openapi,
                });
            }
            Err(SpecError::Unavailable {
                project: project_name.to_string(),
                detail: fetch_err,
            })
        }
    }
}

/// Force a refetch and recache, bypassing all fallbacks.
pub async fn refresh(
    client: &reqwest::Client,
    project_name: &str,
    project: &ProjectConfig,
    cache_dir: &Path,
) -> Result<LoadedSpec, SpecError> {
    let url = spec_url(project);
    let document = fetch_live(client, &url)
        .await
        .map_err(|message| SpecError::Fetch { url, message })?;
    let spec = build(&document)?;
    write_cache(&cache_file(cache_dir, project_name), &document);
    Ok(LoadedSpec {
        spec,
        origin: SpecOrigin::Live,
        document,
    })
}

fn spec_url(project: &ProjectConfig) -> String {
    format!(
        "{}/openapi.json",
        project.base_url.as_str().trim_end_matches('/')
    )
}

async fn fetch_live(client: &reqwest::Client, url: &str) -> Result<Value, String> {
    let response = client.get(url).send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("server returned {status}"));
    }
    response
        .json()
        .await
        .map_err(|e| format!("invalid JSON: {e}"))
}

fn cache_file(cache_dir: &Path, project_name: &str) -> PathBuf {
    cache_dir.join(format!("{project_name}.json"))
}

fn read_cache(path: &Path) -> Option<CacheEntry> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn cache_age(entry: &CacheEntry) -> Option<Duration> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Duration::from_secs(
        now.saturating_sub(entry.fetched_at_unix),
    ))
}

fn write_cache(path: &Path, document: &Value) {
    let entry = CacheEntry {
        fetched_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        openapi: document.clone(),
    };
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&entry)?)?;
        std::fs::rename(&tmp, path)
    };
    if let Err(e) = write() {
        tracing::warn!(path = %path.display(), error = %e, "failed to write spec cache");
    }
}

fn read_spec_file(path: &Path) -> Result<Value, SpecError> {
    let raw = std::fs::read_to_string(path).map_err(|e| SpecError::Unavailable {
        project: String::new(),
        detail: format!("spec_file {}: {e}", path.display()),
    })?;
    serde_json::from_str(&raw)
        .map_err(|e| SpecError::Parse(format!("spec_file {}: {e}", path.display())))
}

/// Build the normalized domain model from a raw OpenAPI document.
pub fn build(document: &Value) -> Result<ApiSpec, SpecError> {
    let raw: RawSpec =
        serde_json::from_value(document.clone()).map_err(|e| SpecError::Parse(e.to_string()))?;
    if raw.paths.is_empty()
        && !document
            .get("paths")
            .map(|p| p.is_object())
            .unwrap_or(false)
    {
        return Err(SpecError::Parse(
            "document has no 'paths' object — is this an OpenAPI spec?".into(),
        ));
    }

    let spec_level_security = raw
        .security
        .as_ref()
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    let mut endpoints = Vec::new();
    for (path, item) in &raw.paths {
        for (method, op) in item.operations() {
            let id = op
                .operation_id
                .clone()
                .unwrap_or_else(|| format!("{method} {path}"));

            let mut params = Vec::new();
            for param_value in item.parameters.iter().chain(op.parameters.iter()) {
                match build_param(document, param_value) {
                    Some(p) => params.push(p),
                    None => tracing::warn!(endpoint = id, "skipping unparseable parameter"),
                }
            }

            let body = op
                .request_body
                .as_ref()
                .and_then(|rb| build_body(document, rb, &id));

            let auth_required = match &op.security {
                Some(reqs) => !reqs.is_empty(),
                None => spec_level_security,
            };

            let responses = op
                .responses
                .iter()
                .map(|(status, response)| build_response(document, status, response))
                .collect();

            endpoints.push(Endpoint {
                id,
                method: method.to_string(),
                path: path.clone(),
                summary: op.summary.clone(),
                description: op.description.clone(),
                tags: op.tags.clone(),
                deprecated: op.deprecated,
                params,
                body,
                auth_required,
                responses,
            });
        }
    }

    let tags = group_tags(&raw, &endpoints);
    Ok(ApiSpec {
        title: raw.info.title,
        version: raw.info.version,
        openapi_version: raw.openapi,
        tags,
        endpoints,
    })
}

fn build_param(document: &Value, param_value: &Value) -> Option<Param> {
    let (resolved, _) = resolve::deref(document, param_value);
    let raw: RawParameter = serde_json::from_value(resolved.clone()).ok()?;
    let location = match raw.location.as_str() {
        "path" => ParamLocation::Path,
        "query" => ParamLocation::Query,
        "header" => ParamLocation::Header,
        // cookie params are out of scope; auth handles its own headers
        _ => return None,
    };
    let normalized = raw
        .schema
        .as_ref()
        .map(|s| normalize::normalize(document, s))
        .unwrap_or_else(normalize::Normalized::any);
    Some(Param {
        name: raw.name,
        location,
        // Path params are always required regardless of what the spec says.
        required: raw.required || location == ParamLocation::Path,
        nullable: normalized.nullable,
        schema: normalized.node,
        default: normalized.default,
        description: raw.description.or(normalized.description),
    })
}

/// Pick the request content type we can drive: JSON preferred, then form
/// variants. Endpoints with only unsupported content degrade to Any-schema JSON.
fn build_body(document: &Value, request_body: &Value, endpoint_id: &str) -> Option<BodySpec> {
    let (resolved, _) = resolve::deref(document, request_body);
    let content = resolved.get("content")?.as_object()?;
    let required = resolved
        .get("required")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let preferred = ["application/json"];
    let acceptable = ["application/x-www-form-urlencoded", "multipart/form-data"];

    let chosen = preferred
        .iter()
        .chain(acceptable.iter())
        .find_map(|ct| content.get_key_value(*ct))
        .or_else(|| content.iter().find(|(ct, _)| ct.contains("json")))
        .or_else(|| {
            tracing::warn!(
                endpoint = endpoint_id,
                content_types = ?content.keys().collect::<Vec<_>>(),
                "no supported request content type; using first declared"
            );
            content.iter().next()
        })?;

    let (content_type, media) = chosen;
    let normalized = media
        .get("schema")
        .map(|s| normalize::normalize(document, s))
        .unwrap_or_else(normalize::Normalized::any);

    Some(BodySpec {
        content_type: content_type.clone(),
        schema: normalized.node,
        required,
    })
}

/// One declared response: description plus the normalized JSON schema when
/// the response declares JSON content.
fn build_response(document: &Value, status: &str, response: &Value) -> crate::model::ResponseSpec {
    let (resolved, _) = resolve::deref(document, response);
    let description = resolved
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    let schema = resolved
        .get("content")
        .and_then(Value::as_object)
        .and_then(|content| {
            content.get("application/json").or_else(|| {
                content
                    .iter()
                    .find(|(ct, _)| ct.contains("json"))
                    .map(|(_, v)| v)
            })
        })
        .and_then(|media| media.get("schema"))
        .map(|s| normalize::normalize(document, s).node);
    crate::model::ResponseSpec {
        status: status.to_string(),
        description,
        schema,
    }
}

/// Tag groups: declared spec order first, then first-seen order from
/// endpoints, with an "untagged" bucket at the end when needed.
fn group_tags(raw: &RawSpec, endpoints: &[Endpoint]) -> Vec<TagGroup> {
    let mut groups: Vec<TagGroup> = raw
        .tags
        .iter()
        .map(|t| TagGroup {
            name: t.name.clone(),
            description: t.description.clone(),
            endpoint_ids: Vec::new(),
        })
        .collect();

    let mut untagged = TagGroup {
        name: "untagged".to_string(),
        description: None,
        endpoint_ids: Vec::new(),
    };

    for endpoint in endpoints {
        if endpoint.tags.is_empty() {
            untagged.endpoint_ids.push(endpoint.id.clone());
            continue;
        }
        for tag in &endpoint.tags {
            match groups.iter_mut().find(|g| &g.name == tag) {
                Some(group) => group.endpoint_ids.push(endpoint.id.clone()),
                None => groups.push(TagGroup {
                    name: tag.clone(),
                    description: None,
                    endpoint_ids: vec![endpoint.id.clone()],
                }),
            }
        }
    }

    groups.retain(|g| !g.endpoint_ids.is_empty());
    if !untagged.endpoint_ids.is_empty() {
        groups.push(untagged);
    }
    groups
}
