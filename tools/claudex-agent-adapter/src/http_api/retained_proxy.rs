use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::RwLock,
    time::Duration,
};

use axum::{
    Json,
    body::Body,
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use crate::launcher::{
    RetainedGeneration, clear_retained, forget_retained_session, read_retained,
};

pub(super) struct RetainedProxy {
    path: PathBuf,
    listen: RwLock<std::net::SocketAddr>,
    pid: RwLock<u32>,
    sessions: RwLock<HashSet<String>>,
    client: reqwest::Client,
}

/// Minimal `/health` view used to decide whether sticky proxying is still safe.
#[derive(Debug, Deserialize)]
struct RetainedHealthProbe {
    #[serde(default)]
    status: String,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    active_http_requests: usize,
    #[serde(default)]
    active_provider_turns: usize,
    #[serde(default)]
    active_subagent_models: BTreeMap<String, usize>,
    #[serde(default)]
    active_claude_session_ids: Vec<String>,
    #[serde(default)]
    busy_claude_session_ids: Vec<String>,
}

impl RetainedHealthProbe {
    fn has_active_work(&self) -> bool {
        self.active_http_requests > 0
            || self.active_provider_turns > 0
            || self.active_subagent_models.values().copied().sum::<usize>() > 0
    }

    fn still_owns(&self, session_id: &str) -> bool {
        // Busy/active session id lists can linger after the retained generation
        // goes quiet (reboot, drained turn). Require live work, matching
        // release_idle_retained, or sticky traffic 502s on a dead ACP.
        if !self.has_active_work() {
            return false;
        }
        if !self.busy_claude_session_ids.is_empty() {
            return self
                .busy_claude_session_ids
                .iter()
                .any(|owned| owned == session_id);
        }
        self.active_claude_session_ids
            .iter()
            .any(|owned| owned == session_id)
    }
}

pub(super) fn proxy_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

impl RetainedProxy {
    pub(super) fn from_path(path: PathBuf, generation: RetainedGeneration) -> Self {
        Self {
            path,
            listen: RwLock::new(generation.listen),
            pid: RwLock::new(generation.pid),
            sessions: RwLock::new(generation.session_ids.into_iter().collect()),
            client: proxy_http_client(),
        }
    }

    pub(super) fn targets(&self, listen: std::net::SocketAddr) -> bool {
        self.refresh();
        self.listen
            .read()
            .map(|current| *current == listen)
            .unwrap_or(false)
    }

    #[cfg(test)]
    pub(super) fn listen_for_test(&self) -> std::net::SocketAddr {
        *self.listen.read().expect("listen")
    }

    fn refresh(&self) {
        // Missing or invalid snapshots must drop sticky ownership. Keeping a
        // stale in-memory map after reboot / operator cleanup made :8318 proxy
        // dead retained listeners and 502 the TUI / SubAgents forever.
        match read_retained(&self.path) {
            Ok(Some(generation)) => self.apply_generation(generation),
            Ok(None) | Err(_) => self.clear_session_memory(),
        }
    }

    fn apply_generation(&self, generation: RetainedGeneration) {
        if let Ok(mut listen) = self.listen.write() {
            *listen = generation.listen;
        }
        if let Ok(mut pid) = self.pid.write() {
            *pid = generation.pid;
        }
        if let Ok(mut sessions) = self.sessions.write() {
            *sessions = generation.session_ids.into_iter().collect();
        }
    }

    fn clear_session_memory(&self) {
        if let Ok(mut sessions) = self.sessions.write() {
            sessions.clear();
        }
    }

    pub(super) fn owns(&self, session_id: &str) -> bool {
        self.refresh();
        self.sessions
            .read()
            .map(|sessions| sessions.contains(session_id))
            .unwrap_or(false)
    }

    fn forget_session(&self, session_id: &str) {
        if self.remove_owned_session(session_id) {
            return;
        }
        let _ = forget_retained_session(&self.path, session_id);
        self.refresh();
    }

    fn remove_owned_session(&self, session_id: &str) -> bool {
        let Ok(mut sessions) = self.sessions.write() else {
            return false;
        };
        sessions.remove(session_id);
        if !sessions.is_empty() {
            return false;
        }
        let _ = clear_retained(&self.path);
        true
    }

    fn clear_all_sessions(&self) {
        if let Ok(mut sessions) = self.sessions.write() {
            sessions.clear();
        }
        let _ = clear_retained(&self.path);
    }

    /// Sticky proxy only while the retained generation is reachable and still
    /// reports this Claude session as busy/active. Idle or dead retained
    /// generations fall through to the live listener instead of 502 loops.
    pub(super) async fn should_proxy_session(&self, session_id: &str) -> bool {
        self.refresh();
        if !self
            .sessions
            .read()
            .map(|sessions| sessions.contains(session_id))
            .unwrap_or(false)
        {
            return false;
        }
        let (listen, expected_pid) = match (self.listen.read(), self.pid.read()) {
            (Ok(listen), Ok(pid)) => (*listen, *pid),
            _ => return false,
        };
        let probe = match self
            .client
            .get(format!("http://{listen}/health"))
            .timeout(Duration::from_millis(400))
            .send()
            .await
        {
            Ok(response) => response.json::<RetainedHealthProbe>().await.ok(),
            Err(_) => None,
        };
        let Some(health) = probe else {
            self.clear_all_sessions();
            return false;
        };
        if health.status != "ok" || health.pid != Some(expected_pid) {
            self.clear_all_sessions();
            return false;
        }
        if health.still_owns(session_id) {
            return true;
        }
        if health.has_active_work() {
            self.forget_session(session_id);
        } else {
            self.clear_all_sessions();
        }
        false
    }

    pub(super) async fn proxy(&self, request: Request) -> Response {
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

pub(super) async fn proxy_request(
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
        Ok(response) => map_upstream_response(response).await,
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": {"message": error.to_string()}})),
        )
            .into_response(),
    }
}

async fn map_upstream_response(response: reqwest::Response) -> Response {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let headers = response.headers().clone();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(8);
    spawn_forward_response_chunks(response, tx);
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

fn spawn_forward_response_chunks(
    response: reqwest::Response,
    tx: tokio::sync::mpsc::Sender<Result<axum::body::Bytes, std::io::Error>>,
) {
    tokio::spawn(forward_response_chunks(response, tx));
}

async fn forward_response_chunks(
    mut response: reqwest::Response,
    tx: tokio::sync::mpsc::Sender<Result<axum::body::Bytes, std::io::Error>>,
) {
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                let _ = tx.send(Err(std::io::Error::other(error))).await;
                break;
            }
        };
        if tx.send(Ok(chunk)).await.is_err() {
            break;
        }
    }
}

pub(super) fn is_hop_by_hop_header(name: &axum::http::HeaderName) -> bool {
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
