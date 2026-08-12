use std::time::Instant;

use super::super::retained_health::seed_recent_from_snapshot;
use super::RetainedProxy;
use crate::launcher::{clear_retained, forget_retained_session, terminate_retained_serve};

impl RetainedProxy {
    pub(super) fn replace_grace_memory_for_generation(
        &self,
        agent_ids: &[String],
        agent_ages: &std::collections::BTreeMap<String, u64>,
    ) {
        self.clear_grace_memory();
        if let Ok(mut recent_agents) = self.recent_agents.write() {
            *recent_agents = seed_recent_from_snapshot(agent_ids, agent_ages, Instant::now());
        }
    }

    pub(super) fn clear_grace_memory(&self) {
        if let Ok(mut last_work_at) = self.last_work_at.write() {
            *last_work_at = None;
        }
        if let Ok(mut recent_agents) = self.recent_agents.write() {
            recent_agents.clear();
        }
    }

    pub(super) fn clear_session_memory(&self) {
        if let Ok(mut sessions) = self.sessions.write() {
            sessions.clear();
        }
        self.clear_grace_memory();
    }

    pub(in crate::http_api) fn owns(&self, session_id: &str) -> bool {
        self.refresh();
        self.owns_cached(session_id)
    }

    pub(in crate::http_api) fn owns_cached(&self, session_id: &str) -> bool {
        self.sessions
            .read()
            .map(|sessions| sessions.contains(session_id))
            .unwrap_or(false)
    }

    pub(super) fn forget_session(&self, session_id: &str) {
        let _ = forget_retained_session(&self.path, session_id);
        self.refresh();
    }

    pub(super) fn clear_all_sessions(&self) {
        let pid = self.pid.read().ok().map(|guard| *guard).unwrap_or(0);
        if let Ok(mut sessions) = self.sessions.write() {
            sessions.clear();
        }
        self.clear_grace_memory();
        let _ = clear_retained(&self.path);
        // Idle/unreachable sticky must not leave an orphan retained daemon
        // until the next ensure() garbage-collects it.
        if pid != 0 {
            terminate_retained_serve(pid);
        }
    }
}

#[cfg(test)]
include!("memory_tests.rs");
