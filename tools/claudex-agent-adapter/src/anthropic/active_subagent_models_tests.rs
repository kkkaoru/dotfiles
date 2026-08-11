use super::*;
use std::time::Duration;

#[test]
fn tracks_overlapping_subagent_occupancy() {
    let registry = Arc::new(ActiveSubagentModels::default());
    let first = registry.acquire("glm-5.2:cloud", Some("agent-a"));
    let second = registry.acquire("glm-5.2:cloud", Some("agent-b"));
    let other = registry.acquire("fugu", None);
    assert_eq!(registry.snapshot()["glm-5.2:cloud"], 2);
    assert_eq!(registry.snapshot()["fugu"], 1);
    assert_eq!(
        registry.active_agent_ids(),
        vec!["agent-a".to_owned(), "agent-b".to_owned()]
    );
    drop(first);
    assert_eq!(registry.snapshot()["glm-5.2:cloud"], 1);
    assert_eq!(registry.active_agent_ids(), vec!["agent-b".to_owned()]);
    drop(second);
    assert!(!registry.snapshot().contains_key("glm-5.2:cloud"));
    assert!(registry.active_agent_ids().is_empty());
    drop(other);
    assert!(registry.snapshot().is_empty());
}

#[test]
fn recent_agent_ages_survive_turn_gaps() {
    let registry = Arc::new(ActiveSubagentModels::default());
    let guard = registry.acquire("glm-5.2:cloud", Some("agent-warm"));
    drop(guard);
    assert!(registry.active_agent_ids().is_empty());
    let ages = registry.recent_agent_ages(Instant::now());
    assert_eq!(ages.get("agent-warm").copied().unwrap_or(u64::MAX), 0);
}

#[test]
fn recent_agent_ages_expire_past_sticky_grace() {
    let registry = Arc::new(ActiveSubagentModels::default());
    {
        let mut recent = registry.recent.lock().expect("recent");
        recent.insert(
            "agent-stale".to_owned(),
            Instant::now() - STICKY_IDLE_GRACE - Duration::from_secs(1),
        );
    }
    assert!(registry.recent_agent_ages(Instant::now()).is_empty());
}

#[test]
fn release_ignores_models_that_were_never_acquired() {
    let registry = Arc::new(ActiveSubagentModels::default());
    registry.release("never-seen", Some("missing-agent"));
    assert!(registry.snapshot().is_empty());
    assert!(registry.active_agent_ids().is_empty());
}

#[test]
fn blank_agent_ids_are_ignored() {
    let registry = Arc::new(ActiveSubagentModels::default());
    let guard = registry.acquire("gpt-test", Some("   "));
    assert!(registry.active_agent_ids().is_empty());
    assert!(registry.recent_agent_ages(Instant::now()).is_empty());
    drop(guard);
}
