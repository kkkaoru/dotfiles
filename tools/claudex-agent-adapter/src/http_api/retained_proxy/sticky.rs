use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use super::super::retained_health::{
    RetainedHealthProbe, agent_still_on_retained, note_retained_activity,
};
use super::{ProbeOutcome, RetainedProxy};

impl RetainedProxy {
    /// Sticky proxy only while the retained generation is reachable and still
    /// reports this Claude session as busy/active (or within a brief idle grace
    /// after live work). Idle or dead retained generations fall through to the
    /// live listener instead of 502 loops.
    ///
    /// When `agent_id` is set and the retained `/health` publishes
    /// `active_subagent_agent_ids`, only those in-flight / recently-seen
    /// SubAgents stay sticky. Newly launched agent IDs run on the live binary
    /// without forgetting the parent session.
    pub(in crate::http_api) async fn should_proxy_session(
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
        match self.probe_health(listen).await {
            ProbeOutcome::Ready(health) => {
                self.decide_sticky_proxy(&health, expected_pid, session_id, agent_id)
            }
            // Slow /health must not terminate warm ACP sessions mid-turn.
            ProbeOutcome::Transient => false,
            ProbeOutcome::Unreachable => {
                self.clear_all_sessions();
                false
            }
        }
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

    async fn probe_health(&self, listen: std::net::SocketAddr) -> ProbeOutcome {
        let response = match self
            .client
            .get(format!("http://{listen}/health"))
            .timeout(Duration::from_millis(400))
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) if error.is_timeout() => return ProbeOutcome::Transient,
            Err(error) if error.is_connect() => return ProbeOutcome::Unreachable,
            // DNS / proxy / protocol noise: keep the generation rather than
            // SIGKILL warm SubAgents on a single ambiguous sample.
            Err(_) => return ProbeOutcome::Transient,
        };
        match response.json::<RetainedHealthProbe>().await {
            Ok(health) => ProbeOutcome::Ready(health),
            Err(_) => ProbeOutcome::Transient,
        }
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
        let local_last_work = self.last_work_at.read().ok().and_then(|guard| *guard);
        let within_grace = health.within_sticky_grace(local_last_work, now);
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
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "sticky_tests.rs"]
mod tests;
