use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context as TaskContext, Poll},
};

use crate::{
    ADAPTER_PROTOCOL_VERSION,
    anthropic::{Bridge, MessagesRequest, RequestIdentity, error_response},
    discovery_model_id, subagent_policy, working_directory,
};
use axum::{
    Json, Router,
    body::{Body, BodyDataStream, Bytes},
    extract::{Request, State},
    http::{HeaderMap, Response, StatusCode},
    middleware,
    middleware::Next,
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::{Value, json};
use tokio_stream::Stream;

mod handover;
mod logging;
mod retained_proxy;
mod web_search;

pub fn http_router(bridge: Arc<Bridge>, model: String, auth_token: Option<String>) -> Router {
    http_router_with_handover(bridge, model, auth_token, None)
}

pub(crate) fn http_router_with_handover(
    bridge: Arc<Bridge>,
    model: String,
    auth_token: Option<String>,
    handover: Option<crate::listen_handover::ListenHandover>,
) -> Router {
    let active_http_requests = Arc::new(AtomicUsize::new(0));
    let health_active_http_requests = Arc::clone(&active_http_requests);
    let active_provider_turns = Arc::new(AtomicUsize::new(0));
    let health_active_provider_turns = Arc::clone(&active_provider_turns);
    let health_model = model;
    let health_bridge = Arc::clone(&bridge);
    let subscription_max_processes = bridge.subscription_max_processes();
    let subscription_timeout_minutes = bridge.subscription_timeout_minutes();
    let backend_routes = bridge.backend_routes();
    let worker_routes = bridge.worker_routes();
    let search_worker_routes = bridge.search_worker_routes();
    let subagent_hard_timeout_seconds = bridge.subagent_hard_timeout_seconds();
    let models = bridge.routed_models();
    let (handover_state, admin) = handover::layer(handover.clone());
    let protected = protected_router(
        models,
        active_provider_turns,
        active_http_requests,
        auth_token,
    )
    .layer(middleware::from_fn_with_state(
        handover_state,
        handover::proxy_retained_sessions,
    ))
    .with_state(Arc::clone(&bridge));
    let handover_for_health = handover;
    Router::new()
        .route(
            "/health",
            get(move || async move {
                let status = if health_bridge.is_alive() {
                    StatusCode::OK
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                };
                let session_ids = health_bridge.active_claude_session_ids().await;
                let busy_session_ids = health_bridge.busy_claude_session_ids().await;
                let listen = handover_for_health
                    .as_ref()
                    .map(|handover| handover.advertised_addr().to_string());
                (
                    status,
                    Json(json!({
                        "status":if status.is_success() { "ok" } else { "unavailable" },
                        "pid":std::process::id(),
                        "protocol_version":ADAPTER_PROTOCOL_VERSION,
                        "build_id":env!("CLAUDEX_BUILD_ID"),
                        "codex_config_fingerprint":std::env::var(crate::app_server::CODEX_CONFIG_FINGERPRINT_ENV).unwrap_or_default(),
                        "service_config_fingerprint":std::env::var(crate::launcher::SERVICE_CONFIG_FINGERPRINT_ENV).unwrap_or_default(),
                        "backend_routes":backend_routes,
                        "worker_routes":worker_routes,
                        "search_worker_routes":search_worker_routes,
                        "started_models":health_bridge.started_models(),
                        "model_concurrency":health_bridge.model_concurrency(),
                        "active_subagent_models":health_bridge.active_subagent_models(),
                        "model":health_model,
                        "session_capacity":health_bridge.session_capacity(),
                        "session_slots_used":health_bridge.used_session_slots(),
                        "active_provider_turns":health_active_provider_turns.load(Ordering::Relaxed),
                        "active_http_requests":health_active_http_requests.load(Ordering::Relaxed),
                        "subscription_max_processes":subscription_max_processes,
                        "subscription_timeout_minutes":subscription_timeout_minutes,
                        "subagent_hard_timeout_seconds":subagent_hard_timeout_seconds,
                        "recovery_generation":crate::launcher::recovery_generation(),
                        "listener_handover":handover_for_health.is_some(),
                        "listen":listen,
                        "active_claude_session_ids":session_ids,
                        "busy_claude_session_ids":busy_session_ids
                    })),
                )
            }),
        )
        .merge(protected)
        .merge(admin)
        .layer(middleware::from_fn(logging::trace_http_request))
}

fn protected_router(
    models: Vec<String>,
    active_provider_turns: Arc<AtomicUsize>,
    active_http_requests: Arc<AtomicUsize>,
    auth_token: Option<String>,
) -> Router<Arc<Bridge>> {
    Router::new()
        .route(
            "/v1/models",
            get(move || async move {
                let data = models
                    .into_iter()
                    .map(|model| {
                        // Claude Code gateway discovery filters to `^(claude|anthropic)`.
                        // Advertise a discovery id while keeping the real model as the label.
                        json!({
                            "id":discovery_model_id(&model),
                            "type":"model",
                            "display_name":model,
                            "description":"Claudex provider model"
                        })
                    })
                    .collect::<Vec<_>>();
                Json(json!({"object":"list","data":data}))
            }),
        )
        .route(
            "/v1/messages",
            post(messages).route_layer(middleware::from_fn_with_state(
                active_provider_turns,
                track_active_provider_turn,
            )),
        )
        .route("/v1/messages/count_tokens", post(count_tokens_handler))
        .route(
            "/v1/code/sessions/{session_id}/worker/web-search",
            post(web_search::ccr_web_search),
        )
        .route_layer(middleware::from_fn_with_state(
            active_http_requests,
            track_active_http_request,
        ))
        .route_layer(middleware::from_fn_with_state(auth_token, authorize))
}

async fn track_active_http_request(
    State(active): State<Arc<AtomicUsize>>,
    request: Request,
    next: Next,
) -> Response<Body> {
    active.fetch_add(1, Ordering::Relaxed);
    let active = ActiveCounter(active);
    hold_active_until_body_complete(next.run(request).await, active)
}

async fn track_active_provider_turn(
    State(active): State<Arc<AtomicUsize>>,
    request: Request,
    next: Next,
) -> Response<Body> {
    active.fetch_add(1, Ordering::Relaxed);
    let active = ActiveCounter(active);
    hold_active_until_body_complete(next.run(request).await, active)
}

fn hold_active_until_body_complete(
    response: Response<Body>,
    active: ActiveCounter,
) -> Response<Body> {
    response.map(|body| {
        Body::from_stream(ActiveBodyStream {
            inner: body.into_data_stream(),
            active: Some(active),
        })
    })
}

struct ActiveBodyStream {
    inner: BodyDataStream,
    active: Option<ActiveCounter>,
}

impl Stream for ActiveBodyStream {
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let item = Pin::new(&mut self.inner).poll_next(cx);
        if matches!(&item, Poll::Ready(None) | Poll::Ready(Some(Err(_)))) {
            self.active.take();
        }
        item
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

struct ActiveCounter(Arc<AtomicUsize>);

impl Drop for ActiveCounter {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
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
    Json(body): Json<Value>,
) -> Response<axum::body::Body> {
    let (mut request, tools_were_provided) = match decode_messages_request(body) {
        Ok(request) => request,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    };
    let identity = match request_identity(&headers) {
        Ok(identity) => identity,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    };
    request.working_directory = request_working_directory(&headers);
    let mut disabled_subagent_models = match subagent_policy::active_models() {
        Ok(models) => models,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    };
    match subagent_policy::request_models(&headers) {
        Ok(models) => disabled_subagent_models.extend(models),
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    }
    request.disabled_subagent_models = disabled_subagent_models;
    bridge
        .messages_with_identity(request, identity, tools_were_provided)
        .await
        .unwrap_or_else(|error| error_response(StatusCode::BAD_GATEWAY, error))
}

fn decode_messages_request(body: Value) -> anyhow::Result<(MessagesRequest, bool)> {
    let tools_were_provided = body
        .as_object()
        .is_some_and(|object| object.contains_key("tools"));
    Ok((serde_json::from_value(body)?, tools_were_provided))
}

const CLAUDE_CODE_SESSION_ID_HEADER: &str = "x-claude-code-session-id";
const CLAUDE_CODE_AGENT_ID_HEADER: &str = "x-claude-code-agent-id";
const CLAUDE_CODE_PARENT_AGENT_ID_HEADER: &str = "x-claude-code-parent-agent-id";
const MAX_CLAUDE_CODE_ID_BYTES: usize = 256;

fn request_identity(headers: &HeaderMap) -> anyhow::Result<RequestIdentity> {
    Ok(RequestIdentity::new(
        request_identity_header(headers, CLAUDE_CODE_SESSION_ID_HEADER)?,
        request_identity_header(headers, CLAUDE_CODE_AGENT_ID_HEADER)?,
        request_identity_header(headers, CLAUDE_CODE_PARENT_AGENT_ID_HEADER)?,
    ))
}

fn request_identity_header(headers: &HeaderMap, name: &str) -> anyhow::Result<Option<String>> {
    let Some(value) = headers.get(name) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| anyhow::anyhow!("Claude Code identity header `{name}` is not valid UTF-8"))?
        .trim();
    anyhow::ensure!(
        !value.is_empty(),
        "Claude Code identity header `{name}` must not be empty"
    );
    anyhow::ensure!(
        value.len() <= MAX_CLAUDE_CODE_ID_BYTES,
        "Claude Code identity header `{name}` exceeds {MAX_CLAUDE_CODE_ID_BYTES} bytes"
    );
    Ok(Some(value.to_owned()))
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

async fn count_tokens_handler(
    State(bridge): State<Arc<Bridge>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response<Body> {
    let (request, tools_were_provided) = match decode_messages_request(body) {
        Ok(request) => request,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    };
    let identity = match request_identity(&headers) {
        Ok(identity) => identity,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, error),
    };
    let input_tokens = bridge.count_tokens_with_identity(request, &identity, tools_were_provided);
    Json(json!({ "input_tokens": input_tokens })).into_response()
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
