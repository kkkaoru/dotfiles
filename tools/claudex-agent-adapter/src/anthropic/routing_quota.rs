//! Live CodexBar / `claudex-route-usage` quota snapshot for SubAgent preflight.
//!
//! Hard-block only true exhaustion: `disabled_subagent_models` and provider
//! reasons `exhausted` / `provider-exhaustion-cooldown`. Low remaining is a
//! selection heuristic for automatic `selected_workers` and must not mark an
//! explicit SubAgent launch as cooling down.

use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const CACHE_FILE: &str = "usage-routing.json";
const DEFAULT_TTL_SECS: f64 = 300.0;
const EXHAUSTED_REASONS: &[&str] = &["exhausted", "provider-exhaustion-cooldown"];

pub(crate) fn cache_path_for_home(home: impl AsRef<Path>) -> PathBuf {
    home.as_ref().join(".cache/claudex").join(CACHE_FILE)
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn current_cache_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(cache_path_for_home)
}

pub(crate) fn summary_marks_model_exhausted(summary: &Value, model: &str) -> bool {
    if summary
        .get("disabled_subagent_models")
        .and_then(Value::as_array)
        .is_some_and(|models| models.iter().any(|value| value.as_str() == Some(model)))
    {
        return true;
    }
    let Some(providers) = summary.get("providers").and_then(Value::as_object) else {
        return false;
    };
    providers.values().any(|fields| {
        let reason = fields.get("reason").and_then(Value::as_str).unwrap_or("");
        EXHAUSTED_REASONS.contains(&reason)
            && fields.get("model").and_then(Value::as_str) == Some(model)
    })
}

pub(crate) fn live_cache_marks_model_exhausted(
    path: Option<&Path>,
    model: &str,
    now: SystemTime,
) -> bool {
    let Some(path) = path else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(cached) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    if !cache_is_fresh(&cached, now) {
        return false;
    }
    cached
        .get("summary")
        .is_some_and(|summary| summary_marks_model_exhausted(summary, model))
}

fn cache_is_fresh(cached: &Value, now: SystemTime) -> bool {
    let Some(created) = cached.get("created_at").and_then(Value::as_f64) else {
        return false;
    };
    let now_secs = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    now_secs - created <= DEFAULT_TTL_SECS
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "routing_quota_tests.rs"]
mod extra_tests;
