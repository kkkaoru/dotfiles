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
pub mod runtime;

pub const ADAPTER_PROTOCOL_VERSION: u64 = 12;

use std::sync::Arc;

use anthropic::{Bridge, MessagesRequest, error_response, token_count};
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, Response, StatusCode},
    middleware,
    middleware::Next,
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::json;

pub fn http_router(bridge: Arc<Bridge>, model: String, auth_token: Option<String>) -> Router {
    let health_model = model;
    let health_bridge = Arc::clone(&bridge);
    let subscription_max_processes = bridge.subscription_max_processes();
    let subscription_timeout_minutes = bridge.subscription_timeout_minutes();
    let backend_routes = bridge.backend_routes();
    let models = bridge.routed_models();
    let protected = Router::new()
        .route(
            "/v1/models",
            get(move || async move {
                let data = models
                    .into_iter()
                    .map(|id| json!({"id":id,"object":"model"}))
                    .collect::<Vec<_>>();
                Json(json!({"object":"list","data":data}))
            }),
        )
        .route("/v1/messages", post(messages))
        .route("/v1/messages/count_tokens", post(count_tokens_handler))
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
                        "started_models":health_bridge.started_models(),
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
    Json(request): Json<MessagesRequest>,
) -> Response<axum::body::Body> {
    bridge
        .messages(request)
        .await
        .unwrap_or_else(|error| error_response(StatusCode::BAD_GATEWAY, error))
}

async fn count_tokens_handler(Json(request): Json<MessagesRequest>) -> impl IntoResponse {
    Json(json!({ "input_tokens": token_count(&request) }))
}
