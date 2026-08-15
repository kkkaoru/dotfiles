use super::*;
use serde_json::json;
use std::time::Duration;

const QWEN: &str = "qwen3.8-max-preview";
const SPARK: &str = "gpt-5.3-codex-spark";
const LUNA: &str = "gpt-5.6-luna";

#[test]
fn exhausted_reason_without_disabled_list_still_marks_the_model() {
    let summary = json!({
        "providers": {
            "qwen": {
                "available": false,
                "reason": "exhausted",
                "model": QWEN
            }
        }
    });
    assert!(summary_marks_model_exhausted(&summary, QWEN));
    assert!(!summary_marks_model_exhausted(&json!({}), QWEN));
}

#[test]
fn live_cache_rejects_invalid_json_and_missing_created_at() {
    let root = tempfile::tempdir().expect("invalid routing cache");
    let path = cache_path_for_home(root.path());
    std::fs::create_dir_all(path.parent().expect("cache dir")).expect("cache dir");
    let now = UNIX_EPOCH + Duration::from_secs(1_000);
    std::fs::write(&path, "{not-json").expect("write invalid cache");
    assert!(!live_cache_marks_model_exhausted(Some(&path), QWEN, now));
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({
            "summary": {
                "providers": {
                    "qwen": {"available": false, "reason": "exhausted", "model": QWEN}
                }
            }
        }))
        .expect("json"),
    )
    .expect("write cache without created_at");
    assert!(!live_cache_marks_model_exhausted(Some(&path), QWEN, now));
}

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
fn live_cache_without_path_or_file_is_not_exhausted() {
    let root = tempfile::tempdir().expect("missing routing cache fixture");
    let missing = root.path().join("missing.json");
    let now = UNIX_EPOCH + Duration::from_secs(1_000);

    assert!(!live_cache_marks_model_exhausted(None, QWEN, now));
    assert!(!live_cache_marks_model_exhausted(Some(&missing), QWEN, now));
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
fn low_remaining_is_not_hard_exhausted_even_with_ample_peers() {
    // Production bug: cursor auto at ~9% remaining was hard-blocked as
    // "cooling down" because peers like grok were ample. Low remaining is a
    // selection heuristic, not launch exhaustion.
    let summary = json!({
        "providers": {
            "cursor": {
                "available": true,
                "reason": "available-cursor-quota",
                "model": "auto",
                "remaining_percent": 9.1965
            },
            "grok": {
                "available": true,
                "reason": "available-grok-quota",
                "model": "grok-4.6",
                "remaining_percent": 66.0
            },
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
    });
    assert!(!summary_marks_model_exhausted(&summary, "auto"));
    assert!(!summary_marks_model_exhausted(&summary, SPARK));
    assert!(!summary_marks_model_exhausted(&summary, LUNA));
}

#[test]
fn live_cache_ignores_low_remaining_cursor_auto() {
    let root = tempfile::tempdir().expect("cursor low remaining fixture");
    let path = cache_path_for_home(root.path());
    std::fs::create_dir_all(path.parent().expect("cache dir")).expect("cache dir");
    let now = UNIX_EPOCH + Duration::from_secs(1_000);
    let body = json!({
        "created_at": 1_000.0,
        "configuration_key": "test",
        "summary": {
            "providers": {
                "cursor": {
                    "available": true,
                    "reason": "available-cursor-quota",
                    "model": "auto",
                    "remaining_percent": 9.1965
                },
                "grok": {
                    "available": true,
                    "reason": "available-grok-quota",
                    "model": "grok-4.6",
                    "remaining_percent": 66.0
                }
            },
            "disabled_subagent_models": []
        }
    });
    std::fs::write(&path, serde_json::to_vec(&body).expect("json")).expect("write");
    assert!(!live_cache_marks_model_exhausted(Some(&path), "auto", now));
}

#[test]
fn provider_exhaustion_cooldown_reason_is_hard_exhausted() {
    let summary = json!({
        "providers": {
            "qwen": {
                "available": false,
                "reason": "provider-exhaustion-cooldown",
                "model": QWEN
            },
            "cursor": {
                "available": true,
                "reason": "available-cursor-quota",
                "model": "auto",
                "remaining_percent": 9.1965
            }
        },
        "disabled_subagent_models": []
    });
    assert!(summary_marks_model_exhausted(&summary, QWEN));
    assert!(!summary_marks_model_exhausted(&summary, "auto"));
}

#[test]
fn live_cache_honors_fresh_provider_exhaustion_cooldown() {
    let root = tempfile::tempdir().expect("provider-exhaustion-cooldown cache");
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
                    "reason": "provider-exhaustion-cooldown",
                    "model": QWEN
                }
            },
            "disabled_subagent_models": []
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
