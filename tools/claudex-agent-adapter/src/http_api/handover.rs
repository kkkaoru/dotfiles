use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;
use serde_json::json;

use crate::launcher::load_retained_from_env;
use crate::listen_handover::ListenHandover;

use super::CLAUDE_CODE_SESSION_ID_HEADER;
use super::retained_proxy::{RetainedProxy, proxy_http_client, proxy_request};

#[derive(Clone)]
pub(super) struct HandoverState {
    pub retained: Option<Arc<RetainedProxy>>,
    pub advertised: Option<ListenHandover>,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct RebindRequest {
    #[serde(default)]
    ephemeral: bool,
    #[serde(default)]
    listen: Option<String>,
}

pub(super) fn layer(handover: Option<ListenHandover>) -> (Option<HandoverState>, Router) {
    let Some(listen) = handover else {
        return (None, Router::new());
    };
    let retained = load_retained_from_env()
        .map(|(path, generation)| RetainedProxy::from_path(path, generation))
        .map(Arc::new);
    let state = HandoverState {
        retained,
        advertised: Some(listen.clone()),
        client: proxy_http_client(),
    };
    let router = Router::new()
        .route("/admin/rebind-listener", post(rebind_listener))
        .with_state(listen);
    (Some(state), router)
}

pub(super) async fn proxy_retained_sessions(
    State(state): State<Option<HandoverState>>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let Some(state) = state.as_ref() else {
        return next.run(request).await;
    };
    let session_id = headers
        .get(CLAUDE_CODE_SESSION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match diverted_service_action(state, session_id, request).await {
        DivertedService::NotDiverted(request) => {
            proxy_or_run_retained(state, session_id, request, next).await
        }
        DivertedService::RunLocal(request) => next.run(request).await,
        DivertedService::Proxy(response) => response,
    }
}

enum DivertedService {
    NotDiverted(Request),
    RunLocal(Request),
    Proxy(Response),
}

async fn diverted_service_action(
    state: &HandoverState,
    session_id: Option<&str>,
    request: Request,
) -> DivertedService {
    let Some(advertised) = state.advertised.as_ref() else {
        return DivertedService::NotDiverted(request);
    };
    let current = advertised.advertised_addr();
    let service = advertised.service_addr();
    if current == service {
        return DivertedService::NotDiverted(request);
    }
    // Old daemon left the client-facing service port. Idle keep-alives
    // must ride to that service listen. A promoted warm-start daemon
    // has service=:8318 and advertised=:8318 after cutover — do not
    // proxy back to the dead warm-start port (that 502'd TUI as
    // http://127.0.0.1:62486/v1/messages).
    if retain_session_locally(state, session_id, current) {
        return DivertedService::RunLocal(request);
    }
    DivertedService::Proxy(proxy_request(&state.client, service, request).await)
}

async fn proxy_or_run_retained(
    state: &HandoverState,
    session_id: Option<&str>,
    request: Request,
    next: Next,
) -> Response {
    let Some(retained) = state.retained.as_ref() else {
        return next.run(request).await;
    };
    let Some(session_id) = session_id else {
        return next.run(request).await;
    };
    if !retained.owns(session_id) {
        return next.run(request).await;
    }
    if retained_targets_advertised(retained, state.advertised.as_ref()) {
        return next.run(request).await;
    }
    if !retained.should_proxy_session(session_id).await {
        return next.run(request).await;
    }
    retained.proxy(request).await
}

fn retained_targets_advertised(
    retained: &RetainedProxy,
    advertised: Option<&ListenHandover>,
) -> bool {
    advertised.is_some_and(|handover| retained.targets(handover.advertised_addr()))
}

fn retain_session_locally(
    state: &HandoverState,
    session_id: Option<&str>,
    current: std::net::SocketAddr,
) -> bool {
    let Some(id) = session_id else {
        return false;
    };
    let Some(retained) = state.retained.as_ref() else {
        return false;
    };
    retained.owns(id) && retained.targets(current)
}

async fn rebind_listener(
    State(handover): State<ListenHandover>,
    Json(body): Json<RebindRequest>,
) -> Response {
    let previous = handover.advertised_addr();
    if let Some(listen) = body.listen.as_deref() {
        let Ok(listen) = listen.parse() else {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {"message": "listen must be a socket address"}})),
            )
                .into_response();
        };
        handover.request_bind(listen);
    } else if body.ephemeral {
        handover.request_ephemeral();
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "ephemeral or listen is required"}})),
        )
            .into_response();
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        let current = handover.advertised_addr();
        if current != previous {
            return Json(json!({"listen": current.to_string()})).into_response();
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    (
        StatusCode::GATEWAY_TIMEOUT,
        Json(json!({"error": {"message": "listener did not rebind in time"}})),
    )
        .into_response()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "handover_tests.rs"]
mod tests;
