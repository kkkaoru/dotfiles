//! Non-blocking last-known-good routing snapshot access for hook invocations.

use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

const MAX_FUTURE_SKEW_SECONDS: f64 = 5.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub summary: Value,
    pub generation: u64,
    pub created_at: f64,
    pub fresh: bool,
}

fn valid_worker(worker: &Value) -> bool {
    worker.is_object()
        && ["agent", "model", "effort"].iter().all(|field| {
            worker
                .get(*field)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        })
}

fn valid_summary(summary: &Value, required_disabled: &BTreeSet<String>) -> bool {
    let Some(workers) = summary.get("selected_workers").and_then(Value::as_array) else {
        return false;
    };
    let Some(disabled) = summary
        .get("disabled_subagent_models")
        .and_then(Value::as_array)
    else {
        return false;
    };
    if !summary.get("providers").is_some_and(Value::is_object)
        || !workers.iter().all(valid_worker)
        || !disabled.iter().all(Value::is_string)
    {
        return false;
    }
    let denied: BTreeSet<&str> = disabled.iter().filter_map(Value::as_str).collect();
    if !required_disabled
        .iter()
        .all(|model| denied.contains(model.as_str()))
    {
        return false;
    }
    let unique_agents: BTreeSet<&str> = workers
        .iter()
        .filter_map(|worker| worker.get("agent").and_then(Value::as_str))
        .collect();
    if unique_agents.len() != workers.len() {
        return false;
    }
    workers.iter().all(|worker| {
        worker
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|model| !denied.contains(model) && !required_disabled.contains(model))
    })
}

/// Read a matching snapshot, retaining a stale summary as last-known-good.
pub fn read(
    path: &Path,
    expected_key: &str,
    required_disabled: &BTreeSet<String>,
    now: f64,
    ttl: i64,
) -> Option<Snapshot> {
    let cached: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if cached.get("configuration_key").and_then(Value::as_str) != Some(expected_key) {
        return None;
    }
    let generation = cached.get("generation")?.as_u64()?;
    let created_at = cached.get("created_at")?.as_f64()?;
    if !created_at.is_finite()
        || created_at < 0.0
        || !now.is_finite()
        || created_at > now + MAX_FUTURE_SKEW_SECONDS
    {
        return None;
    }
    let summary = cached.get("summary")?;
    if !valid_summary(summary, required_disabled) {
        return None;
    }
    Some(Snapshot {
        summary: summary.clone(),
        generation,
        created_at,
        fresh: ttl > 0 && now - created_at <= ttl as f64,
    })
}

/// Serve a matching snapshot, or construct an immediate process-free fallback.
pub fn last_known_good_or_else<F>(
    path: &Path,
    expected_key: &str,
    required_disabled: &BTreeSet<String>,
    now: f64,
    ttl: i64,
    fallback: F,
) -> Result<(Value, bool)>
where
    F: FnOnce() -> Result<Value>,
{
    match read(path, expected_key, required_disabled, now, ttl) {
        Some(snapshot) => Ok((snapshot.summary, snapshot.fresh)),
        None => fallback().map(|summary| (summary, false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(model: &str, disabled: &[&str]) -> Value {
        serde_json::json!({
            "providers": {},
            "selected_workers": [{"agent": "worker", "model": model, "effort": "high"}],
            "disabled_subagent_models": disabled,
        })
    }

    fn write(path: &Path, created_at: f64, key: &str, summary: Value) {
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "created_at": created_at,
                "generation": 1,
                "configuration_key": key,
                "summary": summary,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn matching_snapshot_reports_freshness_without_blocking_refresh() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("usage-routing.json");
        write(&path, 1.0, "current-key", summary("gpt-5.6-luna", &[]));

        let stale = read(&path, "current-key", &BTreeSet::new(), 1_000.0, 300).unwrap();
        assert!(!stale.fresh);
        assert_eq!(
            stale.summary["selected_workers"][0]["model"],
            "gpt-5.6-luna"
        );
        let fresh = read(&path, "current-key", &BTreeSet::new(), 200.0, 300).unwrap();
        assert!(fresh.fresh);
    }

    #[test]
    fn cold_mismatch_or_future_cache_uses_fallback() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("usage-routing.json");
        let (cold, fresh) =
            last_known_good_or_else(&path, "current-key", &BTreeSet::new(), 10.0, 300, || {
                Ok(summary("claude-sonnet-5", &[]))
            })
            .unwrap();
        assert!(!fresh);
        assert_eq!(cold["selected_workers"][0]["model"], "claude-sonnet-5");

        write(&path, 10.0, "old-policy", summary("disabled-model", &[]));
        assert!(read(&path, "current-key", &BTreeSet::new(), 10.0, 300).is_none());
        write(&path, 100.0, "current-key", summary("future-model", &[]));
        assert!(read(&path, "current-key", &BTreeSet::new(), 10.0, 300).is_none());
    }

    #[test]
    fn malformed_or_policy_conflicting_summary_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("usage-routing.json");
        write(
            &path,
            10.0,
            "current-key",
            serde_json::json!({"providers": {}, "selected_workers": {}}),
        );
        assert!(read(&path, "current-key", &BTreeSet::new(), 10.0, 300).is_none());
        write(&path, 10.0, "current-key", summary("allowed-model", &[]));
        let required = BTreeSet::from(["denied-model".to_owned()]);
        assert!(read(&path, "current-key", &required, 10.0, 300).is_none());
        write(
            &path,
            10.0,
            "current-key",
            summary("denied-model", &["denied-model"]),
        );
        assert!(read(&path, "current-key", &required, 10.0, 300).is_none());
    }
}
