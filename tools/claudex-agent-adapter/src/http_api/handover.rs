use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;
use serde_json::json;

use crate::launcher::{RetainedGeneration, load_retained_from_env, read_retained};
use crate::listen_handover::ListenHandover;

use super::CLAUDE_CODE_SESSION_ID_HEADER;

#[derive(Clone)]
pub(super) struct HandoverState {
    pub retained: Option<Arc<RetainedProxy>>,
    pub advertised: Option<ListenHandover>,
    client: reqwest::Client,
}

fn proxy_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

pub(super) struct RetainedProxy {
    path: PathBuf,
    listen: RwLock<std::net::SocketAddr>,
    sessions: RwLock<HashSet<String>>,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct RebindRequest {
    #[serde(default)]
    ephemeral: bool,
    #[serde(default)]
    listen: Option<String>,
}

impl RetainedProxy {
    fn from_path(path: PathBuf, generation: RetainedGeneration) -> Self {
        Self {
            path,
            listen: RwLock::new(generation.listen),
            sessions: RwLock::new(generation.session_ids.into_iter().collect()),
            client: proxy_http_client(),
        }
    }

    fn targets(&self, listen: std::net::SocketAddr) -> bool {
        self.refresh();
        self.listen
            .read()
            .map(|current| *current == listen)
            .unwrap_or(false)
    }

    fn refresh(&self) {
        let Ok(Some(generation)) = read_retained(&self.path) else {
            return;
        };
        if let Ok(mut listen) = self.listen.write() {
            *listen = generation.listen;
        }
        if let Ok(mut sessions) = self.sessions.write() {
            *sessions = generation.session_ids.into_iter().collect();
        }
    }

    fn owns(&self, session_id: &str) -> bool {
        self.refresh();
        self.sessions
            .read()
            .map(|sessions| sessions.contains(session_id))
            .unwrap_or(false)
    }

    async fn proxy(&self, request: Request) -> Response {
        self.refresh();
        let listen = match self.listen.read() {
            Ok(listen) => *listen,
            Err(_) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"error": {"message": "retained listen lock poisoned"}})),
                )
                    .into_response();
            }
        };
        proxy_request(&self.client, listen, request).await
    }
}

async fn proxy_request(
    client: &reqwest::Client,
    listen: std::net::SocketAddr,
    request: Request,
) -> Response {
    let path = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(request.uri().path());
    let url = format!("http://{listen}{path}");
    let mut upstream = client.request(request.method().clone(), url);
    for (name, value) in request.headers() {
        if is_hop_by_hop_header(name) {
            continue;
        }
        upstream = upstream.header(name, value);
    }
    let body = match axum::body::to_bytes(request.into_body(), 32 * 1024 * 1024).await {
        Ok(body) => body,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"message": error.to_string()}})),
            )
                .into_response();
        }
    };
    match upstream.body(body).send().await {
        Ok(mut response) => {
            let status =
                StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let headers = response.headers().clone();
            let (tx, rx) =
                tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(8);
            tokio::spawn(async move {
                loop {
                    match response.chunk().await {
                        Ok(Some(chunk)) => {
                            if tx.send(Ok(chunk)).await.is_err() {
                                break;
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            let _ = tx.send(Err(std::io::Error::other(error))).await;
                            break;
                        }
                    }
                }
            });
            let mut mapped = Response::builder().status(status);
            for (name, value) in headers.iter() {
                mapped = mapped.header(name, value);
            }
            mapped
                .body(Body::from_stream(
                    tokio_stream::wrappers::ReceiverStream::new(rx),
                ))
                .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
        }
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": {"message": error.to_string()}})),
        )
            .into_response(),
    }
}

pub(super) fn layer(handover: Option<ListenHandover>) -> (Option<HandoverState>, Router) {
    let Some(listen) = handover else {
        return (None, Router::new());
    };
    let retained = load_retained_from_env()
        .map(|(path, generation)| RetainedProxy::from_path(path, generation))
        .map(Arc::new);
    let state = HandoverState {
        retained: retained.clone(),
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
    if let Some(advertised) = state.advertised.as_ref() {
        let current = advertised.advertised_addr();
        let service = advertised.service_addr();
        if current != service {
            // Old daemon left the client-facing service port. Idle keep-alives
            // must ride to that service listen. A promoted warm-start daemon
            // has service=:8318 and advertised=:8318 after cutover — do not
            // proxy back to the dead warm-start port (that 502'd TUI as
            // http://127.0.0.1:62486/v1/messages).
            let keep_local = session_id.is_some_and(|id| {
                state
                    .retained
                    .as_ref()
                    .is_some_and(|retained| retained.owns(id) && retained.targets(current))
            });
            if !keep_local {
                return proxy_request(&state.client, service, request).await;
            }
            return next.run(request).await;
        }
    }
    let Some(retained) = state.retained.as_ref() else {
        return next.run(request).await;
    };
    let Some(session_id) = session_id else {
        return next.run(request).await;
    };
    if retained.owns(session_id) {
        if let Some(advertised) = state.advertised.as_ref()
            && retained.targets(advertised.advertised_addr())
        {
            return next.run(request).await;
        }
        return retained.proxy(request).await;
    }
    next.run(request).await
}

fn is_hop_by_hop_header(name: &axum::http::HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
            | "host"
    )
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
