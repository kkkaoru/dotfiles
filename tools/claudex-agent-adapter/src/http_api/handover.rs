use std::{collections::HashSet, sync::Arc};

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

use crate::launcher::{RetainedGeneration, load_retained_from_env};
use crate::listen_handover::ListenHandover;

use super::CLAUDE_CODE_SESSION_ID_HEADER;

#[derive(Clone)]
pub(super) struct HandoverState {
    pub retained: Option<Arc<RetainedProxy>>,
}

pub(super) struct RetainedProxy {
    listen: std::net::SocketAddr,
    sessions: HashSet<String>,
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
    fn from_generation(generation: RetainedGeneration) -> Self {
        Self {
            listen: generation.listen,
            sessions: generation.session_ids.into_iter().collect(),
            client: reqwest::Client::new(),
        }
    }

    fn owns(&self, session_id: &str) -> bool {
        self.sessions.contains(session_id)
    }

    async fn proxy(&self, request: Request) -> Response {
        let path = request
            .uri()
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or(request.uri().path());
        let url = format!("http://{}{path}", self.listen);
        let mut upstream = self.client.request(request.method().clone(), url);
        for (name, value) in request.headers() {
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
                let status = StatusCode::from_u16(response.status().as_u16())
                    .unwrap_or(StatusCode::BAD_GATEWAY);
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
}

pub(super) fn layer(handover: Option<ListenHandover>) -> (Option<HandoverState>, Router) {
    let Some(listen) = handover else {
        return (None, Router::new());
    };
    let retained = load_retained_from_env()
        .map(RetainedProxy::from_generation)
        .map(Arc::new);
    let state = HandoverState {
        retained: retained.clone(),
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
    let Some(retained) = state.as_ref().and_then(|state| state.retained.as_ref()) else {
        return next.run(request).await;
    };
    let Some(session_id) = headers
        .get(CLAUDE_CODE_SESSION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return next.run(request).await;
    };
    if retained.owns(session_id) {
        return retained.proxy(request).await;
    }
    next.run(request).await
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
