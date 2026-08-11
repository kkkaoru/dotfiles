use std::{
    collections::{BTreeMap, HashMap},
    time::Instant,
};

use serde::Deserialize;

use crate::sticky_grace::{STICKY_IDLE_GRACE, within_sticky_idle_grace_secs};

/// Minimal `/health` view used to decide whether sticky proxying is still safe.
#[derive(Debug, Deserialize)]
pub(super) struct RetainedHealthProbe {
    #[serde(default)]
    pub(super) status: String,
    #[serde(default)]
    pub(super) pid: Option<u32>,
    #[serde(default)]
    pub(super) active_http_requests: usize,
    #[serde(default)]
    pub(super) active_provider_turns: usize,
    #[serde(default)]
    pub(super) active_subagent_models: BTreeMap<String, usize>,
    /// Present on builds that publish live SubAgent agent IDs. `None` means the
    /// retained daemon predates this field and sticky must stay session-scoped.
    #[serde(default)]
    pub(super) active_subagent_agent_ids: Option<Vec<String>>,
    /// Seconds since this daemon last observed live work. `None` on older builds.
    #[serde(default)]
    pub(super) idle_seconds: Option<u64>,
    #[serde(default)]
    pub(super) active_claude_session_ids: Vec<String>,
    #[serde(default)]
    pub(super) busy_claude_session_ids: Vec<String>,
}

impl RetainedHealthProbe {
    pub(super) fn has_active_work(&self) -> bool {
        self.active_http_requests > 0
            || self.active_provider_turns > 0
            || self.active_subagent_models.values().copied().sum::<usize>() > 0
    }

    pub(super) fn within_sticky_grace(&self, local_last_work: Option<Instant>, now: Instant) -> bool {
        if let Some(secs) = self.idle_seconds {
            return within_sticky_idle_grace_secs(Some(secs));
        }
        local_last_work.is_some_and(|seen| now.saturating_duration_since(seen) <= STICKY_IDLE_GRACE)
    }

    pub(super) fn still_owns(&self, session_id: &str, within_grace: bool) -> bool {
        if self.has_active_work() {
            return self.session_listed(session_id);
        }
        // Quiet sample: only keep sticky inside the grace window after we last
        // observed live work. Never-seen-work retained daemons still fall through
        // immediately (idle / stale-busy cutover tests).
        within_grace && self.session_listed_or_lists_drained(session_id)
    }

    fn session_listed(&self, session_id: &str) -> bool {
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

    fn session_listed_or_lists_drained(&self, session_id: &str) -> bool {
        if self.busy_claude_session_ids.is_empty() && self.active_claude_session_ids.is_empty() {
            // Counters drained mid-turn before session lists refresh.
            return true;
        }
        self.session_listed(session_id)
    }
}

pub(super) fn agent_still_on_retained(
    health: &RetainedHealthProbe,
    agent_id: Option<&str>,
    recent_agents: &HashMap<String, Instant>,
    now: Instant,
) -> bool {
    let Some(agent_id) = agent_id.filter(|id| !id.is_empty()) else {
        return true;
    };
    let Some(active_agents) = health.active_subagent_agent_ids.as_ref() else {
        return true;
    };
    if active_agents.iter().any(|owned| owned == agent_id) {
        return true;
    }
    // Empty / missing id during a turn gap: keep sticky only for agents we
    // recently saw on this retained generation. Unknown ids still go live.
    recent_agents
        .get(agent_id)
        .is_some_and(|seen| now.saturating_duration_since(*seen) <= STICKY_IDLE_GRACE)
}

pub(super) fn note_retained_activity(
    health: &RetainedHealthProbe,
    last_work_at: &mut Option<Instant>,
    recent_agents: &mut HashMap<String, Instant>,
    now: Instant,
) {
    if health.has_active_work() {
        *last_work_at = Some(now);
    }
    let Some(active_agents) = health.active_subagent_agent_ids.as_ref() else {
        return;
    };
    for agent_id in active_agents {
        if !agent_id.is_empty() {
            recent_agents.insert(agent_id.clone(), now);
        }
    }
    recent_agents.retain(|_, seen| now.saturating_duration_since(*seen) <= STICKY_IDLE_GRACE);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(
        requests: usize,
        busy: &[&str],
        agents: Option<&[&str]>,
        idle_seconds: Option<u64>,
    ) -> RetainedHealthProbe {
        RetainedHealthProbe {
            status: "ok".to_owned(),
            pid: Some(1),
            active_http_requests: requests,
            active_provider_turns: requests,
            active_subagent_models: BTreeMap::new(),
            active_subagent_agent_ids: agents
                .map(|ids| ids.iter().map(|id| (*id).to_owned()).collect()),
            idle_seconds,
            active_claude_session_ids: busy.iter().map(|id| (*id).to_owned()).collect(),
            busy_claude_session_ids: busy.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    #[test]
    fn still_owns_uses_grace_for_quiet_listed_sessions() {
        let quiet = probe(0, &["session-a"], Some(&[]), Some(10));
        assert!(!quiet.still_owns("session-a", false));
        assert!(quiet.still_owns("session-a", true));
        assert!(!quiet.still_owns("session-b", true));
    }

    #[test]
    fn agent_sticky_remembers_recent_ids_across_empty_snapshots() {
        let now = Instant::now();
        let mut recent = HashMap::new();
        recent.insert("agent-old".to_owned(), now);
        let drained = probe(1, &["parent"], Some(&[]), Some(0));
        assert!(agent_still_on_retained(
            &drained,
            Some("agent-old"),
            &recent,
            now
        ));
        assert!(!agent_still_on_retained(
            &drained,
            Some("agent-new"),
            &recent,
            now
        ));
    }

    #[test]
    fn published_idle_seconds_drive_grace_without_local_clock() {
        let within = probe(0, &["session-a"], Some(&[]), Some(10));
        assert!(within.within_sticky_grace(None, Instant::now()));
        let expired = probe(0, &["session-a"], Some(&[]), Some(60));
        assert!(!expired.within_sticky_grace(None, Instant::now()));
    }
}
