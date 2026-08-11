use std::{
    collections::BTreeMap,
    time::Instant,
};

use serde::Deserialize;

use crate::sticky_grace::{STICKY_IDLE_GRACE, within_sticky_idle_grace_secs};

mod seed;
pub(super) use seed::{
    agent_still_on_retained, note_retained_activity, seed_recent_from_snapshot,
};
#[cfg(test)]
#[allow(unused_imports)]
use seed::seed_recent_agents;

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

    pub(super) fn within_sticky_grace(&self, local_last_work: Option<Instant>, now: Instant) -> bool {
        let from_health = within_sticky_idle_grace_secs(self.idle_seconds);
        let from_local = local_last_work
            .is_some_and(|seen| now.saturating_duration_since(seen) <= STICKY_IDLE_GRACE);
        // Prefer either signal: published idle_seconds can lag if /health was not
        // probed during work, while local observation can miss remote retained daemons.
        from_health || from_local
    }

    pub(super) fn still_owns(&self, session_id: &str, within_grace: bool) -> bool {
        if self.has_active_work() {
            // busy_claude_session_ids can name only the sessions currently
            // turning; co-retained quiet sessions still live on active list.
            return self.session_listed(session_id) || self.session_active_listed(session_id);
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
        self.session_active_listed(session_id)
    }

    fn session_active_listed(&self, session_id: &str) -> bool {
        self.active_claude_session_ids
            .iter()
            .any(|owned| owned == session_id)
    }

    fn session_listed_or_lists_drained(&self, session_id: &str) -> bool {
        if self.busy_claude_session_ids.is_empty() && self.active_claude_session_ids.is_empty() {
            // Counters drained mid-turn before session lists refresh.
            return true;
        }
        self.session_listed(session_id) || self.session_active_listed(session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, time::Duration};

    fn probe(
        requests: usize,
        busy: &[&str],
        agents: Option<&[&str]>,
        idle_seconds: Option<u64>,
    ) -> RetainedHealthProbe {
        probe_with_recent(requests, busy, agents, BTreeMap::new(), idle_seconds)
    }

    fn probe_with_recent(
        requests: usize,
        busy: &[&str],
        agents: Option<&[&str]>,
        recent: BTreeMap<String, u64>,
        idle_seconds: Option<u64>,
    ) -> RetainedHealthProbe {
        probe_split(requests, busy, busy, agents, recent, idle_seconds)
    }

    fn probe_split(
        requests: usize,
        busy: &[&str],
        active: &[&str],
        agents: Option<&[&str]>,
        recent: BTreeMap<String, u64>,
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
            recent_subagent_agent_ids: recent,
            idle_seconds,
            active_claude_session_ids: active.iter().map(|id| (*id).to_owned()).collect(),
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
    fn still_owns_keeps_co_retained_session_while_sibling_is_busy() {
        let shared = probe_split(
            1,
            &["other"],
            &["session-a", "other"],
            Some(&[]),
            BTreeMap::new(),
            Some(0),
        );
        assert!(
            shared.still_owns("session-a", true),
            "quiet co-retained session must stay sticky while a sibling turns"
        );
        assert!(shared.still_owns("other", true));
        assert!(!shared.still_owns("session-unknown", true));
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
    fn published_recent_ages_keep_sticky_without_local_memory() {
        let now = Instant::now();
        let mut ages = BTreeMap::new();
        ages.insert("agent-warm".to_owned(), 5);
        let drained = probe_with_recent(0, &["parent"], Some(&[]), ages, Some(5));
        assert!(agent_still_on_retained(
            &drained,
            Some("agent-warm"),
            &HashMap::new(),
            now
        ));
        assert!(!agent_still_on_retained(
            &drained,
            Some("agent-new"),
            &HashMap::new(),
            now
        ));
    }

    #[test]
    fn note_retained_activity_does_not_refresh_seeded_stamps_from_ages() {
        let now = Instant::now();
        let seeded = now - Duration::from_secs(20);
        let mut recent = HashMap::new();
        recent.insert("agent-warm".to_owned(), seeded);
        let mut ages = BTreeMap::new();
        ages.insert("agent-warm".to_owned(), 1);
        let drained = probe_with_recent(0, &["parent"], Some(&[]), ages, Some(1));
        let mut last_work = None;
        note_retained_activity(&drained, &mut last_work, &mut recent, now);
        assert_eq!(
            recent.get("agent-warm").copied(),
            Some(seeded),
            "published ages must not reset an older local sticky stamp to now"
        );
    }

    #[test]
    fn seed_recent_agents_skips_blanks() {
        let now = Instant::now();
        let seeded = seed_recent_agents(
            &["agent-a".to_owned(), "".to_owned(), "agent-b".to_owned()],
            now,
        );
        assert_eq!(seeded.len(), 2);
        assert!(seeded.contains_key("agent-a"));
        assert!(seeded.contains_key("agent-b"));
    }

    #[test]
    fn published_idle_seconds_drive_grace_without_local_clock() {
        let within = probe(0, &["session-a"], Some(&[]), Some(10));
        assert!(within.within_sticky_grace(None, Instant::now()));
        let expired = probe(0, &["session-a"], Some(&[]), Some(60));
        assert!(!expired.within_sticky_grace(None, Instant::now()));
        assert!(
            expired.within_sticky_grace(Some(Instant::now()), Instant::now()),
            "local observation must still keep grace when published idle is stale"
        );
    }
}
