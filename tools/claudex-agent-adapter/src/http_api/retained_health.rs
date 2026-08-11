use std::{collections::BTreeMap, time::Instant};

use serde::Deserialize;

use crate::sticky_grace::{STICKY_IDLE_GRACE, within_sticky_idle_grace_secs};

mod seed;
#[cfg(test)]
#[allow(unused_imports)]
use seed::seed_recent_agents;
pub(super) use seed::{agent_still_on_retained, note_retained_activity, seed_recent_from_snapshot};

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
    /// Warm SubAgent agentIds → seconds since last observation. Absent on older builds.
    #[serde(default)]
    pub(super) recent_subagent_agent_ids: BTreeMap<String, u64>,
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

    pub(super) fn within_sticky_grace(
        &self,
        local_last_work: Option<Instant>,
        now: Instant,
    ) -> bool {
        let from_health = within_sticky_idle_grace_secs(self.idle_seconds);
        let from_local = local_last_work
            .is_some_and(|seen| now.saturating_duration_since(seen) <= STICKY_IDLE_GRACE);
        // Prefer either signal: published idle_seconds can lag if /health was not
        // probed during work, while local observation can miss remote retained daemons.
        from_health || from_local
    }

    pub(super) fn still_owns(&self, session_id: &str, within_grace: bool) -> bool {
        if self.has_active_work() {
            // HTTP counters can rise before session lists refresh; treat empty
            // lists as a registration race and keep sticky ownership.
            return self.session_listed_or_lists_drained(session_id);
        }
        // Quiet sample: only keep sticky inside the grace window after we last
        // observed live work. Never-seen-work retained daemons still fall through
        // immediately (idle / stale-busy cutover tests).
        within_grace && self.session_listed_or_lists_drained(session_id)
    }

    pub(super) fn session_listed(&self, session_id: &str) -> bool {
        if !self.busy_claude_session_ids.is_empty() {
            return self
                .busy_claude_session_ids
                .iter()
                .any(|owned| owned == session_id);
        }
        self.session_active_listed(session_id)
    }

    pub(super) fn session_active_listed(&self, session_id: &str) -> bool {
        self.active_claude_session_ids
            .iter()
            .any(|owned| owned == session_id)
    }

    pub(super) fn session_listed_or_lists_drained(&self, session_id: &str) -> bool {
        if self.busy_claude_session_ids.is_empty() && self.active_claude_session_ids.is_empty() {
            // Counters drained mid-turn before session lists refresh.
            return true;
        }
        self.session_listed(session_id) || self.session_active_listed(session_id)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "retained_health_tests.rs"]
mod tests;
