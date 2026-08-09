//! Live CodexBar / `claudex-route-usage` quota snapshot for SubAgent preflight.
//!
//! Automatic selection already drops exhausted providers, but multi-SubAgent
//! generation still hydrates a stale `selected_workers` list and prefers Qwen
//! after Cline empty-ACP. Consult the same snapshot before the ACP call.

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
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    const QWEN: &str = "qwen3.8-max-preview";

    #[test]
    fn marks_disabled_list_and_exhausted_provider_reason() {
        let summary = json!({
            "providers": {
                "qwen": {
                    "available": false,
                    "reason": "exhausted",
                    "model": QWEN
                },
                "cursor": {
                    "available": true,
                    "reason": "available-cursor-quota",
                    "model": "auto"
                }
            },
            "disabled_subagent_models": [QWEN]
        });
        assert!(summary_marks_model_exhausted(&summary, QWEN));
        assert!(!summary_marks_model_exhausted(&summary, "auto"));
    }

    #[test]
    fn ignores_missing_codexbar_quota_as_not_exhausted() {
        let summary = json!({
            "providers": {
                "qwen": {
                    "available": false,
                    "reason": "missing",
                    "model": QWEN
                }
            },
            "disabled_subagent_models": []
        });
        assert!(!summary_marks_model_exhausted(&summary, QWEN));
    }

    #[test]
    fn live_cache_honors_fresh_exhausted_snapshot() {
        let root = tempfile::tempdir().expect("routing cache fixture");
        let path = cache_path_for_home(root.path());
        std::fs::create_dir_all(path.parent().expect("cache dir")).expect("cache dir");
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let body = json!({
            "created_at": 1_000.0,
            "configuration_key": "test",
            "summary": {
                "providers": {
                    "qwen": {
                        "available": false,
                        "reason": "exhausted",
                        "model": QWEN
                    }
                },
                "disabled_subagent_models": [QWEN]
            }
        });
        std::fs::write(&path, serde_json::to_vec(&body).expect("json")).expect("write");
        assert!(live_cache_marks_model_exhausted(Some(&path), QWEN, now));
        assert!(!live_cache_marks_model_exhausted(Some(&path), "auto", now));
    }

    #[test]
    fn live_cache_ignores_stale_snapshot() {
        let root = tempfile::tempdir().expect("stale cache fixture");
        let path = cache_path_for_home(root.path());
        std::fs::create_dir_all(path.parent().expect("cache dir")).expect("cache dir");
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let body = json!({
            "created_at": 1_000.0 - DEFAULT_TTL_SECS - 1.0,
            "configuration_key": "test",
            "summary": {
                "providers": {
                    "qwen": {
                        "available": false,
                        "reason": "exhausted",
                        "model": QWEN
                    }
                },
                "disabled_subagent_models": [QWEN]
            }
        });
        std::fs::write(&path, serde_json::to_vec(&body).expect("json")).expect("write");
        assert!(!live_cache_marks_model_exhausted(Some(&path), QWEN, now));
    }
}
