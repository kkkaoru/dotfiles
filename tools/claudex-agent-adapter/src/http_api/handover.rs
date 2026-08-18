use std::sync::Arc;

use axum::{
    Router,
    extract::{Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
    routing::post,
};
use serde::Deserialize;

use crate::launcher::load_retained_from_env;
use crate::listen_handover::ListenHandover;

use super::CLAUDE_CODE_SESSION_ID_HEADER;
use super::handover_circuit::{self, HandoverCircuit};
use super::retained_proxy::{
    ProxyOutcome, RetainedProxy, listen_accepts_health, proxy_http_client, proxy_request,
};

const CLAUDE_CODE_AGENT_ID_HEADER: &str = "x-claude-code-agent-id";

#[path = "handover_helpers.rs"]
mod helpers;
use helpers::{rebind_listener, retain_session_locally, retained_targets_advertised};

#[derive(Clone)]
pub(super) struct HandoverState {
    pub retained: Option<Arc<RetainedProxy>>,
    pub advertised: Option<ListenHandover>,
    client: reqwest::Client,
    circuit: Arc<HandoverCircuit>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RebindRequest {
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
        circuit: Arc::new(HandoverCircuit::default()),
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
    let agent_id = headers
        .get(CLAUDE_CODE_AGENT_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match diverted_service_action(state, session_id, request).await {
        DivertedService::NotDiverted(request) => {
            proxy_or_run_retained(state, session_id, agent_id, request, next).await
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
    if recover_open_circuit(state, session_id) {
        return DivertedService::RunLocal(request);
    }
    if !listen_accepts_health(&state.client, service).await {
        return DivertedService::RunLocal(request);
    }
    match proxy_request(&state.client, service, request).await {
        ProxyOutcome::Response(response) => {
            DivertedService::Proxy(observe_proxy(state, session_id, response))
        }
        // Race after Ready: one transport 503 is acceptable. Do not increment
        // the circuit — the next request re-probes and stays local.
        ProxyOutcome::TransportFailed(message) => {
            DivertedService::Proxy(handover_circuit::retry_response(message))
        }
    }
}

async fn proxy_or_run_retained(
    state: &HandoverState,
    session_id: Option<&str>,
    agent_id: Option<&str>,
    request: Request,
    next: Next,
) -> Response {
    let Some(retained) = state.retained.as_ref() else {
        return next.run(request).await;
    };
    let Some(session_id) = session_id else {
        return next.run(request).await;
    };
    // One disk snapshot per request — owns/targets/proxy used to each refresh.
    retained.refresh();
    if !retained.owns_cached(session_id) {
        return next.run(request).await;
    }
    if retained_targets_advertised(retained, state.advertised.as_ref()) {
        return next.run(request).await;
    }
    if !retained.should_proxy_session(session_id, agent_id).await {
        return next.run(request).await;
    }
    if recover_open_circuit(state, Some(session_id)) {
        return next.run(request).await;
    }
    match retained.proxy_outcome(request).await {
        ProxyOutcome::TransportFailed(message) => {
            retained.clear_all_sessions();
            handover_circuit::retry_response(message)
        }
        ProxyOutcome::Response(response) => {
            let response = observe_proxy(state, Some(session_id), response);
            if state.circuit.is_open(session_id) {
                retained.forget_session(session_id);
            }
            response
        }
    }
}

fn recover_open_circuit(state: &HandoverState, session_id: Option<&str>) -> bool {
    let Some(session_id) = session_id else {
        return false;
    };
    if !state.circuit.is_open(session_id) {
        return false;
    }
    state.circuit.clear(session_id);
    true
}

#[cfg(test)]
fn open_circuit_response(state: &HandoverState, session_id: Option<&str>) -> Option<Response> {
    let session_id = session_id?;
    if !state.circuit.is_open(session_id) {
        return None;
    }
    Some(handover_circuit::terminal_response(format!(
        "handover proxy circuit open for session {session_id}"
    )))
}

fn observe_proxy(state: &HandoverState, session_id: Option<&str>, response: Response) -> Response {
    if !handover_circuit::is_retry_status(response.status()) {
        return response;
    }
    let Some(session_id) = session_id else {
        return response;
    };
    if state.circuit.note_failure(session_id) {
        return handover_circuit::terminal_response(format!(
            "handover proxy circuit open for session {session_id}"
        ));
    }
    response
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "handover_tests.rs"]
mod tests;
