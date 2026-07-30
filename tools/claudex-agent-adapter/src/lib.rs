#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, allow(unused_features))]

pub mod agent_backend;
pub mod anthropic;
pub mod app_server;
pub mod build_support;
pub mod copilot_acp;
pub mod coverage_gate;
pub mod grok_acp;
pub mod launcher;
pub mod parallel_scheduler;
pub mod path_env;
pub mod provider_config;
pub mod runtime;
mod subagent_policy;
mod web_search;
mod working_directory;

pub const ADAPTER_PROTOCOL_VERSION: u64 = 23;
pub(crate) const NONINTERACTIVE_CHILD_ENV: &str = "CLAUDEX_NONINTERACTIVE_CHILD";
/// Legacy gateway aliases remain accepted on requests from older Claude Code sessions.
pub(crate) const DISCOVERY_MODEL_PREFIX: &str = "claude-claudex-";

use std::sync::Arc;

use anthropic::{Bridge, MessagesRequest, error_response, token_count};
use axum::{
    Json, Router,
    extract::{Path, Request, State},
    http::{HeaderMap, Response, StatusCode},
    middleware,
    middleware::Next,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn http_router(bridge: Arc<Bridge>, model: String, auth_token: Option<String>) -> Router {
    let health_model = model;
    let health_bridge = Arc::clone(&bridge);
    let subscription_max_processes = bridge.subscription_max_processes();
    let subscription_timeout_minutes = bridge.subscription_timeout_minutes();
    let backend_routes = bridge.backend_routes();
    let worker_routes = bridge.worker_routes();
    let search_worker_routes = bridge.search_worker_routes();
    let models = bridge.routed_models();
    let protected = Router::new()
        .route(
            "/v1/models",
            get(move || async move {
                let data = models
                    .into_iter()
                    .map(|model| {
                        json!({
                            "id":model,
                            "type":"model",
                            "display_name":model,
                            "description":"Claudex provider model"
                        })
                    })
                    .collect::<Vec<_>>();
                Json(json!({"object":"list","data":data}))
            }),
        )
        .route("/v1/messages", post(messages))
        .route("/v1/messages/count_tokens", post(count_tokens_handler))
        .route(
            "/v1/code/sessions/{session_id}/worker/web-search",
            post(ccr_web_search),
        )
        .route_layer(middleware::from_fn_with_state(auth_token, authorize));
    Router::new()
        .route(
            "/health",
            get(move || async move {
                let status = if health_bridge.is_alive() {
                    StatusCode::OK
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                };
                (
                    status,
                    Json(json!({
                        "status":if status.is_success() { "ok" } else { "unavailable" },
                        "pid":std::process::id(),
                        "protocol_version":ADAPTER_PROTOCOL_VERSION,
                        "build_id":env!("CLAUDEX_BUILD_ID"),
                        "backend_routes":backend_routes,
                        "worker_routes":worker_routes,
                        "search_worker_routes":search_worker_routes,
                        "started_models":health_bridge.started_models(),
                        "model_concurrency":health_bridge.model_concurrency(),
                        "model":health_model,
                        "session_capacity":health_bridge.session_capacity(),
                        "session_slots_used":health_bridge.used_session_slots(),
                        "subscription_max_processes":subscription_max_processes,
                        "subscription_timeout_minutes":subscription_timeout_minutes
                    })),
                )
            }),
        )
        .merge(protected)
        .with_state(bridge)
}

async fn authorize(
    State(expected): State<Option<String>>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response<axum::body::Body>, StatusCode> {
    if expected
        .as_deref()
        .is_none_or(|token| has_token(&headers, token))
    {
        return Ok(next.run(request).await);
    }
    Err(StatusCode::UNAUTHORIZED)
}

fn has_token(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get("x-api-key")
        .is_some_and(|value| value.as_bytes() == expected.as_bytes())
        || headers
            .get("authorization")
            .is_some_and(|value| value.as_bytes() == format!("Bearer {expected}").as_bytes())
}

async fn messages(
    State(bridge): State<Arc<Bridge>>,
    headers: HeaderMap,
    Json(mut request): Json<MessagesRequest>,
) -> Response<axum::body::Body> {
    request.working_directory = request_working_directory(&headers);
    request.disabled_subagent_models = match subagent_policy::request_models(&headers) {
        Ok(models) => models,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    };
    bridge
        .messages(request)
        .await
        .unwrap_or_else(|error| error_response(StatusCode::BAD_GATEWAY, error))
}

fn request_working_directory(headers: &HeaderMap) -> Option<std::path::PathBuf> {
    let path = headers
        .get(working_directory::HEADER_NAME)
        .and_then(|value| value.to_str().ok())
        .and_then(working_directory::decode)?
        .canonicalize()
        .ok()?;
    path.is_dir().then_some(path)
}

async fn count_tokens_handler(Json(request): Json<MessagesRequest>) -> impl IntoResponse {
    Json(json!({ "input_tokens": token_count(&request) }))
}

#[derive(Debug, Default, Deserialize)]
struct CcrWebSearchRequest {
    query: String,
    #[serde(default)]
    allowed_domains: Vec<String>,
    #[serde(default)]
    blocked_domains: Vec<String>,
}

async fn ccr_web_search(
    State(bridge): State<Arc<Bridge>>,
    Path(session_id): Path<String>,
    Json(request): Json<CcrWebSearchRequest>,
) -> Response<axum::body::Body> {
    if session_id.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            anyhow::anyhow!("session_id is empty"),
        );
    }
    match bridge.run_web_search(&request.query).await {
        Ok(mut response) => {
            response.results.retain(|result| {
                domain_allowed(
                    &result.url,
                    &request.allowed_domains,
                    &request.blocked_domains,
                )
            });
            response.search_count = u64::try_from(response.results.len()).unwrap_or(u64::MAX);
            Json(json!({"results": response.results, "error": Value::Null})).into_response()
        }
        Err(error) => error_response(StatusCode::BAD_GATEWAY, error),
    }
}

fn domain_allowed(url: &str, allowed: &[String], blocked: &[String]) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return false;
    }
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    let matches = |domain: &String| {
        let domain = domain.trim_start_matches(".").to_ascii_lowercase();
        host == domain || host.ends_with(&format!(".{domain}"))
    };
    !blocked.iter().any(matches) && (allowed.is_empty() || allowed.iter().any(matches))
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_valid_existing_working_directory_headers() {
        let root = tempfile::tempdir().expect("working directory fixture");
        let canonical = root.path().canonicalize().expect("canonical fixture");
        let mut headers = HeaderMap::new();
        headers.insert(
            working_directory::HEADER_NAME,
            working_directory::encode(root.path())
                .parse()
                .expect("header"),
        );
        assert_eq!(request_working_directory(&headers), Some(canonical));

        headers.insert(
            working_directory::HEADER_NAME,
            "/definitely/missing".parse().expect("missing header"),
        );
        assert!(request_working_directory(&headers).is_none());
        assert!(request_working_directory(&HeaderMap::new()).is_none());
    }

    #[test]
    fn parses_terminal_subagent_policy_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            subagent_policy::HEADER_NAME,
            "gpt-5.6-sol,grok-4.5".parse().unwrap(),
        );
        assert_eq!(
            subagent_policy::request_models(&headers)
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["gpt-5.6-sol", "grok-4.5"]
        );
    }

    #[test]
    fn applies_domain_filters_to_search_urls() {
        let allowed = vec!["example.com".to_owned()];
        let blocked = vec!["blocked.example.com".to_owned()];
        assert!(domain_allowed("https://example.com/a", &allowed, &[]));
        assert!(domain_allowed("https://sub.example.com/a", &allowed, &[]));
        assert!(!domain_allowed("https://other.com/a", &allowed, &[]));
        assert!(!domain_allowed(
            "https://blocked.example.com/a",
            &[],
            &blocked
        ));
        assert!(!domain_allowed("not-a-url", &[], &[]));
    }
}
