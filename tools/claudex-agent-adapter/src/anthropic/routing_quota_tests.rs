use super::*;
use serde_json::json;
use std::time::Duration;

const QWEN: &str = "qwen3.8-max-preview";
const SPARK: &str = "gpt-5.3-codex-spark";

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
