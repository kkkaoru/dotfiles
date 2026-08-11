use super::health::Health;

pub(in crate::launcher) fn retained_session_ids(health: &Health) -> Vec<String> {
    // Quiet co-retained sessions must survive cutover with a busy sibling so
    // sticky proxy can keep their warm SubAgents / prompt-cache. Ownership is
    // re-checked after promote and released once sticky idle grace expires.
    if !health.has_active_work() && !health.within_sticky_idle_grace() {
        return Vec::new();
    }
    health
        .busy_claude_session_ids
        .iter()
        .chain(health.active_claude_session_ids.iter())
        .filter(|id| !id.is_empty())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(in crate::launcher) fn warm_agent_ages(
    health: &Health,
) -> std::collections::BTreeMap<String, u64> {
    let mut ages = std::collections::BTreeMap::new();
    for id in &health.active_subagent_agent_ids {
        if !id.is_empty() {
            ages.insert(id.clone(), 0);
        }
    }
    for (id, age) in &health.recent_subagent_agent_ids {
        if id.is_empty() {
            continue;
        }
        // Prefer in-flight age 0 over a stale published age for the same id.
        ages.entry(id.clone()).or_insert(*age);
    }
    ages
}

#[cfg(test)]
pub(in crate::launcher) fn warm_agent_ids(health: &Health) -> Vec<String> {
    warm_agent_ages(health).into_keys().collect()
}
