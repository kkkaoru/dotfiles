use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Instant,
};

use tokio::sync::Semaphore;

use axum::extract::Request;
#[cfg(test)]
use axum::response::Response;

use super::retained_health::seed_recent_from_snapshot;
use crate::launcher::{RetainedGeneration, read_retained};

mod forward;
mod memory;
mod sticky;

#[cfg(test)]
pub(super) use forward::HANDOVER_HOP_HEADER;
#[cfg(test)]
pub(super) use forward::is_hop_by_hop_header;
pub(super) use forward::{ProxyOutcome, handover_hop_count, listen_accepts_health, proxy_request};

pub(super) const MAX_PROXY_IN_FLIGHT: usize = 32;

pub(super) struct RetainedProxy {
    path: PathBuf,
    listen: RwLock<std::net::SocketAddr>,
    pid: RwLock<u32>,
    sessions: RwLock<HashSet<String>>,
    last_work_at: RwLock<Option<Instant>>,
    recent_agents: RwLock<HashMap<String, Instant>>,
    client: reqwest::Client,
    slots: Arc<Semaphore>,
}

pub(super) fn proxy_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .pool_max_idle_per_host(32)
        .pool_idle_timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
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
            slots: Arc::new(Semaphore::new(MAX_PROXY_IN_FLIGHT)),
        }
    }

    pub(super) fn with_proxy_slots(mut self, slots: Arc<Semaphore>) -> Self {
        self.slots = slots;
        self
    }

    pub(super) fn try_proxy_slot(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        Arc::clone(&self.slots).try_acquire_owned().ok()
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

    #[cfg(test)]
    pub(super) async fn proxy(&self, request: Request) -> Response {
        self.proxy_outcome(request).await.into_response()
    }

    pub(in crate::http_api) async fn proxy_outcome(&self, request: Request) -> ProxyOutcome {
        let listen = match self.listen.read() {
            Ok(listen) => *listen,
            Err(_) => {
                return ProxyOutcome::TransportFailed("retained listen lock poisoned".to_owned());
            }
        };
        proxy_request(&self.client, listen, request).await
    }
}
