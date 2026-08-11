use std::{
    sync::{Arc, Mutex, atomic::AtomicUsize},
    time::Instant,
};

use crate::{anthropic::Bridge, discovery_model_id};
use axum::{Json, Router, middleware, routing::{get, post}};
use serde_json::json;

#[cfg(test)]
use axum::{
    body::{Body, Bytes},
    http::HeaderMap,
};
#[cfg(test)]
use crate::{subagent_policy, working_directory};

mod active;
mod handover;
mod health_route;
mod logging;
mod retained_health;
mod retained_proxy;
mod web_search;
mod messages_handlers;
use messages_handlers::{
    authorize, count_tokens_handler, messages, request_identity, CLAUDE_CODE_SESSION_ID_HEADER,
};
#[cfg(test)]
use messages_handlers::{
    CLAUDE_CODE_AGENT_ID_HEADER, CLAUDE_CODE_PARENT_AGENT_ID_HEADER, MAX_CLAUDE_CODE_ID_BYTES,
    decode_messages_request, request_working_directory,
};
use active::{ActiveWorkState, track_active_http_request, track_active_provider_turn};

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
    let active_provider_turns = Arc::new(AtomicUsize::new(0));
    let last_work_at = Arc::new(Mutex::new(Instant::now()));
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
        Arc::clone(&active_provider_turns),
        Arc::clone(&active_http_requests),
        Arc::clone(&last_work_at),
        auth_token,
    )
    .layer(middleware::from_fn_with_state(
        handover_state,
        handover::proxy_retained_sessions,
    ))
    .with_state(Arc::clone(&bridge));
    health_route::mount_health_route(
        Router::new(),
        health_route::HealthRouteState {
            bridge: Arc::clone(&bridge),
            model,
            active_http_requests,
            active_provider_turns,
            last_work_at,
            subscription_max_processes,
            subscription_timeout_minutes,
            backend_routes,
            worker_routes,
            search_worker_routes,
            subagent_hard_timeout_seconds,
            handover,
        },
    )
    .merge(protected)
    .merge(admin)
    .layer(middleware::from_fn(logging::trace_http_request))
}

fn protected_router(
    models: Vec<String>,
    active_provider_turns: Arc<AtomicUsize>,
    active_http_requests: Arc<AtomicUsize>,
    last_work_at: Arc<Mutex<Instant>>,
    auth_token: Option<String>,
) -> Router<Arc<Bridge>> {
    let provider_work = ActiveWorkState {
        counter: active_provider_turns,
        last_work_at: Arc::clone(&last_work_at),
    };
    let http_work = ActiveWorkState {
        counter: active_http_requests,
        last_work_at,
    };
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
                provider_work,
                track_active_provider_turn,
            )),
        )
        .route("/v1/messages/count_tokens", post(count_tokens_handler))
        .route(
            "/v1/code/sessions/{session_id}/worker/web-search",
            post(web_search::ccr_web_search),
        )
        .route_layer(middleware::from_fn_with_state(
            http_work,
            track_active_http_request,
        ))
        .route_layer(middleware::from_fn_with_state(auth_token, authorize))
}


#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
