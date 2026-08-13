use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::RwLock,
    time::Instant,
};

use axum::{extract::Request, response::Response};

use super::retained_health::seed_recent_from_snapshot;
use crate::launcher::{RetainedGeneration, read_retained};

mod forward;
mod memory;
mod sticky;

#[cfg(test)]
pub(super) use forward::is_hop_by_hop_header;
pub(super) use forward::proxy_request;

enum ProbeOutcome {
    Ready(super::retained_health::RetainedHealthProbe),
    /// Connect refused / dead listen: clear sticky ownership.
    Unreachable,
    /// Timeout or undecodable body: fall through without killing the generation.
    Transient,
}

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
        let now = Instant::now();
        Self {
            path,
            listen: RwLock::new(generation.listen),
            pid: RwLock::new(generation.pid),
            sessions: RwLock::new(generation.session_ids.into_iter().collect()),
            last_work_at: RwLock::new(None),
            recent_agents: RwLock::new(seed_recent_from_snapshot(
                &generation.agent_ids,
                &generation.agent_ages,
                now,
            )),
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
        let pid_changed = self
            .pid
            .read()
            .ok()
            .is_none_or(|current| *current != generation.pid);
        let agent_ids = generation.agent_ids;
        let agent_ages = generation.agent_ages;
        if let Ok(mut listen) = self.listen.write() {
            *listen = generation.listen;
        }
        if let Ok(mut pid) = self.pid.write() {
            *pid = generation.pid;
        }
        if let Ok(mut sessions) = self.sessions.write() {
            *sessions = generation.session_ids.into_iter().collect();
        }
        // Do not carry sticky grace / agent memory across retained generations.
        if pid_changed {
            self.replace_grace_memory_for_generation(&agent_ids, &agent_ages);
        }
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

    pub(super) async fn proxy(&self, request: Request) -> Response {
        let listen = match self.listen.read() {
            Ok(listen) => *listen,
            Err(_) => {
                return crate::http_api::handover_circuit::retry_response(
                    "retained listen lock poisoned".to_owned(),
                );
            }
        };
        proxy_request(&self.client, listen, request).await
    }
}
