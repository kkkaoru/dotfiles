//! Live CodexBar / `claudex-route-usage` quota snapshot for SubAgent preflight.
//!
//! Automatic selection already drops exhausted or low-remaining providers, but
//! explicit SubAgent launches still hydrate a stale `selected_workers` list
//! (Qwen after Cline empty-ACP, Spark after weekly remaining < 25%). Consult
//! the same snapshot before the provider call.

use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const CACHE_FILE: &str = "usage-routing.json";
const DEFAULT_TTL_SECS: f64 = 300.0;
const EXHAUSTED_REASONS: &[&str] = &["exhausted", "provider-exhaustion-cooldown"];
/// Match `claudex-route-usage` automatic selection: below this is depleted.
const LOW_REMAINING_PERCENT: f64 = 25.0;
/// At least one peer at or above this lets low-remaining models be skipped.
const AMPLE_REMAINING_PERCENT: f64 = 40.0;

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
    if providers.values().any(|fields| {
        let reason = fields.get("reason").and_then(Value::as_str).unwrap_or("");
        EXHAUSTED_REASONS.contains(&reason)
            && fields.get("model").and_then(Value::as_str) == Some(model)
    }) {
        return true;
    }
    summary_marks_model_low_remaining(summary, model)
}

fn number_f64(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_i64().map(|n| n as f64))
}

pub(crate) fn provider_selection_remaining(fields: &Value) -> Option<f64> {
    if let Some(remaining) = fields.get("remaining_percent").and_then(number_f64) {
        return Some(remaining);
    }
    let windows = fields.get("quota_windows")?;
    let weekly = windows.get("seven-day").and_then(number_f64);
    let five_hour = windows.get("five-hour").and_then(number_f64);
    match (weekly, five_hour) {
        (Some(weekly), Some(five_hour)) => Some(weekly.min(five_hour)),
        (Some(weekly), None) => Some(weekly),
        (None, Some(five_hour)) => Some(five_hour),
        (None, None) => None,
    }
}

fn quota_field_values(summary: &Value) -> impl Iterator<Item = &Value> {
    let providers = summary
        .get("providers")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|fields| fields.values());
    let native = summary
        .get("native_worker_quota")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|fields| fields.values());
    providers.chain(native)
}

fn summary_has_ample_remaining(summary: &Value) -> bool {
    quota_field_values(summary).any(|fields| {
        provider_selection_remaining(fields)
            .is_some_and(|remaining| remaining >= AMPLE_REMAINING_PERCENT)
    })
}

fn summary_marks_model_low_remaining(summary: &Value, model: &str) -> bool {
    if !summary_has_ample_remaining(summary) {
        return false;
    }
    let Some(providers) = summary.get("providers").and_then(Value::as_object) else {
        return false;
    };
    providers.values().any(|fields| {
        fields.get("model").and_then(Value::as_str) == Some(model)
            && provider_selection_remaining(fields)
                .is_some_and(|remaining| remaining < LOW_REMAINING_PERCENT)
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
    const SPARK: &str = "gpt-5.3-codex-spark";
    const LUNA: &str = "gpt-5.6-luna";

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
    fn low_remaining_spark_is_exhausted_when_ample_peers_exist() {
        // Historical bug: CodexBar still marked spark `available-codex-quota`
        // at 17% weekly remaining, so explicit `claudex-gpt-spark` launches
        // kept starting after automatic selected_workers already dropped it.
        let summary = json!({
            "providers": {
                "codex-spark": {
                    "available": true,
                    "reason": "available-codex-quota",
                    "model": SPARK,
                    "remaining_percent": 17.0,
                    "quota_windows": {"five-hour": null, "seven-day": 17.0}
                },
                "codex": {
                    "available": true,
                    "reason": "available-codex-quota",
                    "model": LUNA,
                    "remaining_percent": 98.0,
                    "quota_windows": {"five-hour": null, "seven-day": 98.0}
                }
            },
            "disabled_subagent_models": []
        });
        assert!(
            summary_marks_model_exhausted(&summary, SPARK),
            "spark below 25% must not keep launching beside ample luna"
        );
        assert!(!summary_marks_model_exhausted(&summary, LUNA));
    }

    #[test]
    fn low_remaining_spark_stays_eligible_when_no_ample_peer_exists() {
        let summary = json!({
            "providers": {
                "codex-spark": {
                    "available": true,
                    "reason": "available-codex-quota",
                    "model": SPARK,
                    "remaining_percent": 17.0
                },
                "cursor": {
                    "available": true,
                    "reason": "available-cursor-quota",
                    "model": "auto",
                    "remaining_percent": 30.0
                }
            },
            "disabled_subagent_models": []
        });
        assert!(
            !summary_marks_model_exhausted(&summary, SPARK),
            "without a >=40% peer, keep_all still allows the low spark worker"
        );
    }

    #[test]
    fn ample_spark_is_not_exhausted() {
        let summary = json!({
            "providers": {
                "codex-spark": {
                    "available": true,
                    "reason": "available-codex-quota",
                    "model": SPARK,
                    "remaining_percent": 50.0
                },
                "codex": {
                    "available": true,
                    "reason": "available-codex-quota",
                    "model": LUNA,
                    "remaining_percent": 98.0
                }
            },
            "disabled_subagent_models": []
        });
        assert!(!summary_marks_model_exhausted(&summary, SPARK));
    }

    #[test]
    fn five_hour_window_can_deplete_spark_beside_native_headroom() {
        let summary = json!({
            "providers": {
                "codex-spark": {
                    "available": true,
                    "reason": "available-codex-quota",
                    "model": SPARK,
                    "quota_windows": {"five-hour": 10.0, "seven-day": 80.0}
                }
            },
            "native_worker_quota": {
                "claudex-sonnet": {
                    "available": true,
                    "remaining_percent": 94.0
                }
            },
            "disabled_subagent_models": []
        });
        assert!(
            summary_marks_model_exhausted(&summary, SPARK),
            "min(weekly, five-hour) below 25% must skip spark when sonnet is ample"
        );
    }

    #[test]
    fn live_cache_marks_low_remaining_spark() {
        let root = tempfile::tempdir().expect("spark cache fixture");
        let path = cache_path_for_home(root.path());
        std::fs::create_dir_all(path.parent().expect("cache dir")).expect("cache dir");
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let body = json!({
            "created_at": 1_000.0,
            "configuration_key": "test",
            "summary": {
                "providers": {
                    "codex-spark": {
                        "available": true,
                        "reason": "available-codex-quota",
                        "model": SPARK,
                        "remaining_percent": 17.0
                    },
                    "codex": {
                        "available": true,
                        "reason": "available-codex-quota",
                        "model": LUNA,
                        "remaining_percent": 98.0
                    }
                },
                "disabled_subagent_models": []
            }
        });
        std::fs::write(&path, serde_json::to_vec(&body).expect("json")).expect("write");
        assert!(live_cache_marks_model_exhausted(Some(&path), SPARK, now));
        assert!(!live_cache_marks_model_exhausted(Some(&path), LUNA, now));
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
