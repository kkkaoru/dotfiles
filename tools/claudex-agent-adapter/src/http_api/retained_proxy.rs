use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::RwLock,
    time::{Duration, Instant},
};

use axum::{
    Json,
    body::Body,
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

use super::retained_health::{
    RetainedHealthProbe, agent_still_on_retained, note_retained_activity, within_sticky_grace,
};
use crate::launcher::{
    RetainedGeneration, clear_retained, forget_retained_session, read_retained,
    terminate_retained_serve,
};

pub(super) struct RetainedProxy {
    path: PathBuf,
    listen: RwLock<std::net::SocketAddr>,
    pid: RwLock<u32>,
    sessions: RwLock<HashSet<String>>,
    last_work_at: RwLock<Option<Instant>>,
    recent_agents: RwLock<HashMap<String, Instant>>,
    client: reqwest::Client,
}

pub(super) fn proxy_http_client() -> reqwest::Client {
    // Keep a normal idle pool: sticky proxy /health and SSE reuse the same
    // retained listen for the life of a parent session.
    reqwest::Client::new()
}

impl RetainedProxy {
    pub(super) fn from_path(path: PathBuf, generation: RetainedGeneration) -> Self {
        Self {
            path,
            listen: RwLock::new(generation.listen),
            pid: RwLock::new(generation.pid),
            sessions: RwLock::new(generation.session_ids.into_iter().collect()),
            last_work_at: RwLock::new(None),
            recent_agents: RwLock::new(HashMap::new()),
            client: proxy_http_client(),
        }
    }

    pub(super) fn targets(&self, listen: std::net::SocketAddr) -> bool {
        self.refresh();
        self.targets_cached(listen)
    }

    pub(super) fn targets_cached(&self, listen: std::net::SocketAddr) -> bool {
        self.listen
            .read()
            .map(|current| *current == listen)
            .unwrap_or(false)
    }

    #[cfg(test)]
    pub(super) fn listen_for_test(&self) -> std::net::SocketAddr {
        *self.listen.read().expect("listen")
    }

    pub(super) fn refresh(&self) {
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
        self.owns_cached(session_id)
    }

    pub(super) fn owns_cached(&self, session_id: &str) -> bool {
        self.sessions
            .read()
            .map(|sessions| sessions.contains(session_id))
            .unwrap_or(false)
    }

    fn forget_session(&self, session_id: &str) {
        let _ = forget_retained_session(&self.path, session_id);
        self.refresh();
    }

    fn clear_all_sessions(&self) {
        let pid = self.pid.read().ok().map(|guard| *guard).unwrap_or(0);
        if let Ok(mut sessions) = self.sessions.write() {
            sessions.clear();
        }
        let _ = clear_retained(&self.path);
        // Idle/unreachable sticky must not leave an orphan retained daemon
        // until the next ensure() garbage-collects it.
        if pid != 0 {
            terminate_retained_serve(pid);
        }
    }

    /// Sticky proxy only while the retained generation is reachable and still
    /// reports this Claude session as busy/active (or within a brief idle grace
    /// after live work). Idle or dead retained generations fall through to the
    /// live listener instead of 502 loops.
    ///
    /// When `agent_id` is set and the retained `/health` publishes
    /// `active_subagent_agent_ids`, only those in-flight / recently-seen
    /// SubAgents stay sticky. Newly launched agent IDs run on the live binary
    /// without forgetting the parent session.
    pub(super) async fn should_proxy_session(
        &self,
        session_id: &str,
        agent_id: Option<&str>,
    ) -> bool {
        if !self.tracks_session(session_id) {
            return false;
        }
        let Some((listen, expected_pid)) = self.listen_and_pid() else {
            return false;
        };
        let Some(health) = self.probe_health(listen).await else {
            self.clear_all_sessions();
            return false;
        };
        self.decide_sticky_proxy(&health, expected_pid, session_id, agent_id)
    }

    fn tracks_session(&self, session_id: &str) -> bool {
        self.sessions
            .read()
            .map(|sessions| sessions.contains(session_id))
            .unwrap_or(false)
    }

    fn listen_and_pid(&self) -> Option<(std::net::SocketAddr, u32)> {
        match (self.listen.read(), self.pid.read()) {
            (Ok(listen), Ok(pid)) => Some((*listen, *pid)),
            _ => None,
        }
    }

    async fn probe_health(&self, listen: std::net::SocketAddr) -> Option<RetainedHealthProbe> {
        let response = self
            .client
            .get(format!("http://{listen}/health"))
            .timeout(Duration::from_millis(400))
            .send()
            .await
            .ok()?;
        response.json::<RetainedHealthProbe>().await.ok()
    }

    fn decide_sticky_proxy(
        &self,
        health: &RetainedHealthProbe,
        expected_pid: u32,
        session_id: &str,
        agent_id: Option<&str>,
    ) -> bool {
        if health.status != "ok" || health.pid != Some(expected_pid) {
            self.clear_all_sessions();
            return false;
        }
        let now = Instant::now();
        self.note_probe(health, now);
        let within_grace = self
            .last_work_at
            .read()
            .ok()
            .is_some_and(|guard| within_sticky_grace(*guard, now));
        if !health.still_owns(session_id, within_grace) {
            self.release_unowned_session(session_id, health.has_active_work() || within_grace);
            return false;
        }
        let recent = self.recent_agents.read().ok();
        agent_still_on_retained(
            health,
            agent_id,
            recent.as_deref().unwrap_or(&HashMap::new()),
            now,
        )
    }

    #[cfg(test)]
    pub(super) fn mark_recent_work_for_test(&self) {
        if let Ok(mut last_work_at) = self.last_work_at.write() {
            *last_work_at = Some(Instant::now());
        }
    }

    #[cfg(test)]
    pub(super) fn remember_agent_for_test(&self, agent_id: &str) {
        if let Ok(mut recent_agents) = self.recent_agents.write() {
            recent_agents.insert(agent_id.to_owned(), Instant::now());
        }
    }

    fn note_probe(&self, health: &RetainedHealthProbe, now: Instant) {
        let Ok(mut last_work_at) = self.last_work_at.write() else {
            return;
        };
        let Ok(mut recent_agents) = self.recent_agents.write() else {
            return;
        };
        note_retained_activity(health, &mut last_work_at, &mut recent_agents, now);
    }

    fn release_unowned_session(&self, session_id: &str, keep_generation: bool) {
        if keep_generation {
            self.forget_session(session_id);
        } else {
            self.clear_all_sessions();
        }
    }

    pub(super) async fn proxy(&self, request: Request) -> Response {
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
