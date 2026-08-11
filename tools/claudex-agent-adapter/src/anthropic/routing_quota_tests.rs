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
fn low_remaining_without_providers_object_is_not_exhausted() {
    let summary = json!({
        "native_worker_quota": {
            "cursor": {"remaining_percent": 80.0, "model": "auto"}
        },
        "providers": "not-an-object"
    });
    assert!(!summary_marks_model_exhausted(&summary, SPARK));
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
