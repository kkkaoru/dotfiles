use std::{
    collections::{BTreeMap, HashMap},
    time::{Duration, Instant},
};

use crate::sticky_grace::{STICKY_IDLE_GRACE, STICKY_IDLE_GRACE_SECS};

use super::RetainedHealthProbe;

pub(in crate::http_api) fn agent_still_on_retained(
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
    if health
        .recent_subagent_agent_ids
        .get(agent_id)
        .is_some_and(|age| *age <= STICKY_IDLE_GRACE_SECS)
    {
        return true;
    }
    // Empty / missing id during a turn gap: keep sticky only for agents we
    // recently saw on this retained generation. Unknown ids still go live.
    recent_agents
        .get(agent_id)
        .is_some_and(|seen| now.saturating_duration_since(*seen) <= STICKY_IDLE_GRACE)
}

pub(in crate::http_api) fn note_retained_activity(
    health: &RetainedHealthProbe,
    last_work_at: &mut Option<Instant>,
    recent_agents: &mut HashMap<String, Instant>,
    now: Instant,
) {
    if health.has_active_work() {
        *last_work_at = Some(now);
    }
    let Some(active_agents) = health.active_subagent_agent_ids.as_ref() else {
        // Legacy retained builds omit agent id fields entirely.
        return;
    };
    for agent_id in active_agents {
        if !agent_id.is_empty() {
            recent_agents.insert(agent_id.clone(), now);
        }
    }
    // Published ages are absolute last-seen times. Do not refresh existing
    // local stamps to `now` on every probe or sticky grace never expires.
    for (agent_id, age) in &health.recent_subagent_agent_ids {
        if agent_id.is_empty() {
            continue;
        }
        let seen_at = now
            .checked_sub(Duration::from_secs(*age))
            .unwrap_or(now);
        recent_agents.entry(agent_id.clone()).or_insert(seen_at);
    }
    recent_agents.retain(|_, seen| now.saturating_duration_since(*seen) <= STICKY_IDLE_GRACE);
}

/// Seed sticky memory from promote-time agentIds so cutover does not forget
/// warm SubAgents before the first retained /health sample refreshes them.
pub(in crate::http_api) fn seed_recent_agents(
    agent_ids: &[String],
    now: Instant,
) -> HashMap<String, Instant> {
    agent_ids
        .iter()
        .filter(|id| !id.is_empty())
        .map(|id| (id.clone(), now))
        .collect()
}

/// Prefer published ages so promote does not reset remaining sticky grace to
/// a full window. Legacy snapshots without ages fall back to `agent_ids` at now.
pub(in crate::http_api) fn seed_recent_from_snapshot(
    agent_ids: &[String],
    agent_ages: &BTreeMap<String, u64>,
    now: Instant,
) -> HashMap<String, Instant> {
    if agent_ages.is_empty() {
        return seed_recent_agents(agent_ids, now);
    }
    seed_recent_agents_with_ages(agent_ages, now)
}

pub(in crate::http_api) fn seed_recent_agents_with_ages(
    agent_ages: &BTreeMap<String, u64>,
    now: Instant,
) -> HashMap<String, Instant> {
    agent_ages
        .iter()
        .filter(|(id, _)| !id.is_empty())
        .map(|(id, age)| {
            let seen_at = now
                .checked_sub(Duration::from_secs(*age))
                .unwrap_or(now);
            (id.clone(), seen_at)
        })
        .collect()
}
