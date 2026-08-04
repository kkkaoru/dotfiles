use super::concurrency::apply_model_concurrency;
use super::orchestration::task_fanout;
use super::quota::codexbar_window_quota_entry;
use super::summary::routing_summary;
use super::workers::{
    default_subagent_route, enforce_worker_model_separation, worker_capacity_metadata,
};
use super::{memory_parallel_cap, pressure_level};
use crate::config::{Config, load_config};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tempfile::NamedTempFile;

fn config_from_json(raw: &str) -> Config {
    let file = NamedTempFile::new().unwrap();
    std::fs::write(file.path(), raw).unwrap();
    load_config(file.path()).unwrap()
}

fn sample_config() -> Config {
    config_from_json(
        r#"{
          "version": 1,
          "mainProviders": ["codex"],
          "providers": [
            {
              "id": "codex",
              "agent": "claudex-gpt-spark",
              "defaultModel": "gpt-5.6-sol",
              "subagentModel": "gpt-5.3-codex-spark",
              "effort": "high",
              "enabled": true,
              "usageProvider": "codex",
              "modelPrefixes": ["gpt"],
              "backend": "codex-app-server"
            },
            {
              "id": "grok",
              "agent": "claudex-grok",
              "defaultModel": "grok-4.5",
              "effort": "high",
              "enabled": true,
              "usageProvider": "grok",
              "modelPrefixes": ["grok"],
              "backend": "grok-acp"
            },
            {
              "id": "qwen",
              "agent": "claudex-qwen",
              "defaultModel": "qwen3.8-max-preview",
              "effort": "high",
              "enabled": true,
              "usageProvider": "qwencloud",
              "modelPrefixes": ["qwen"],
              "backend": "configured-acp"
            }
          ],
          "fallback": {
            "agent": "claudex-sonnet",
            "model": "claude-sonnet-5",
            "effort": "high"
          },
          "nativeWorkers": [],
          "advisor": {
            "agent": "custom-advisor",
            "model": "claude-fable-5",
            "effort": "xhigh"
          }
        }"#,
    )
}

/// Two Codex providers sharing one usage provider, where the spark worker
/// reads its weekly window from `extraRateWindows`.
fn spark_weekly_config() -> Config {
    config_from_json(
        r#"{
          "version": 1,
          "mainProviders": ["codex", "codex-spark"],
          "providers": [
            {
              "id": "codex",
              "agent": "claudex-gpt",
              "defaultModel": "gpt-5.6-luna",
              "effort": "max",
              "enabled": true,
              "usageProvider": "codex",
              "modelPrefixes": ["gpt"],
              "backend": "codex-app-server"
            },
            {
              "id": "codex-spark",
              "agent": "claudex-gpt-spark",
              "defaultModel": "gpt-5.3-codex-spark",
              "subagentModel": "gpt-5.3-codex-spark",
              "effort": "xhigh",
              "enabled": true,
              "usageProvider": "codex",
              "usageWeeklyWindowId": "codex-spark-weekly",
              "modelPrefixes": ["gpt-5.3-codex-spark"],
              "backend": "codex-app-server"
            }
          ],
          "fallback": {
            "agent": "claudex-sonnet",
            "model": "claude-sonnet-5",
            "effort": "high"
          },
          "nativeWorkers": [],
          "advisor": {
            "agent": "custom-advisor",
            "model": "claude-fable-5",
            "effort": "xhigh"
          }
        }"#,
    )
}

/// Native Claude workers that share CodexBar's single `claude` usage entry.
fn native_claude_config() -> Config {
    config_from_json(
        r#"{
          "version": 1,
          "mainProviders": ["codex"],
          "providers": [
            {
              "id": "codex",
              "agent": "claudex-gpt",
              "defaultModel": "gpt-5.6-luna",
              "effort": "max",
              "enabled": true,
              "usageProvider": "codex",
              "modelPrefixes": ["gpt"],
              "backend": "codex-app-server"
            }
          ],
          "fallback": {
            "agent": "claudex-sonnet",
            "model": "claude-sonnet-5",
            "effort": "high"
          },
          "nativeWorkers": [
            {
              "agent": "claudex-haiku-search",
              "model": "claude-haiku-4-5",
              "effort": "max",
              "usageProvider": "claude"
            },
            {
              "agent": "claudex-sonnet",
              "model": "claude-sonnet-5",
              "effort": "high",
              "usageProvider": "claude"
            }
          ]
        }"#,
    )
}

fn spark_report(weekly_used_percent: f64) -> Value {
    json!([
        {
          "provider": "codex",
          "usage": {
            "primary": {"usedPercent": 10.0},
            "secondary": {"usedPercent": 86.0},
            "extraRateWindows": [
              {
                "id": "codex-spark-weekly",
                "title": "Codex Spark Weekly",
                "window": {"usedPercent": weekly_used_percent, "windowMinutes": 10080}
              }
            ]
          }
        }
    ])
}

fn report() -> Value {
    json!([
        {"provider":"codex","usage":{"primary":{"usedPercent":10},"secondary":{"usedPercent":20}}},
        {"provider":"grok","usage":{"primary":{"usedPercent":40}}},
        {
          "provider":"qwencloud",
          "usage":{
            "primary":{"usedPercent":20.0,"windowMinutes":300},
            "secondary":{"usedPercent":30.0,"windowMinutes":10080}
          }
        }
    ])
}

fn selected_models(summary: &Value) -> Vec<&str> {
    summary["selected_workers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|worker| worker.get("model").and_then(Value::as_str))
        .collect()
}

fn selected_agents(summary: &Value) -> Vec<&str> {
    summary["selected_agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect()
}

#[test]
fn selects_available_workers_and_exposes_orchestration() {
    let summary = routing_summary(&report(), &sample_config(), &BTreeSet::new()).unwrap();
    let agents = selected_agents(&summary);
    assert!(agents.contains(&"claudex-qwen"));
    assert!(agents.contains(&"claudex-gpt-spark"));
    assert_eq!(summary["orchestration"]["dynamic_fanout"], true);
    assert_eq!(summary["orchestration"]["hook_launches_agents"], false);
    assert_eq!(summary["orchestration"]["task_fanout_default"], 1);
}

#[test]
fn weekly_remaining_orders_workers() {
    let usage = json!([
        {
          "provider":"codex",
          "available": true,
          "reason": "available",
          "maxUsedPercent": 50,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":90},
            {"name":"seven-day","remainingPercent":40}
          ]
        },
        {
          "provider":"grok",
          "available": true,
          "reason": "available",
          "maxUsedPercent": 10,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":10},
            {"name":"seven-day","remainingPercent":80}
          ]
        },
        {"provider":"qwencloud","available":false,"reason":"exhausted","maxUsedPercent":100}
    ]);
    let summary = routing_summary(&usage, &sample_config(), &BTreeSet::new()).unwrap();
    assert_eq!(summary["selected_workers"][0]["agent"], "claudex-grok");
    assert_eq!(summary["selected_workers"][1]["agent"], "claudex-gpt-spark");
}

#[test]
fn drops_low_weekly_workers_when_ample_alternatives_exist() {
    let usage = json!([
        {
          "provider":"codex",
          "available": true,
          "reason": "available",
          "maxUsedPercent": 20,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":80},
            {"name":"seven-day","remainingPercent":80}
          ]
        },
        {
          "provider":"grok",
          "available": true,
          "reason": "available",
          "maxUsedPercent": 90,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":10},
            {"name":"seven-day","remainingPercent":10}
          ]
        },
        {
          "provider":"qwencloud",
          "available": true,
          "reason": "available",
          "maxUsedPercent": 5,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":95},
            {"name":"seven-day","remainingPercent":95}
          ]
        }
    ]);
    let summary = routing_summary(&usage, &sample_config(), &BTreeSet::new()).unwrap();
    let agents = selected_agents(&summary);
    assert!(agents.contains(&"claudex-qwen"));
    assert!(agents.contains(&"claudex-gpt-spark"));
    assert!(
        !agents.contains(&"claudex-grok"),
        "low weekly grok must leave automatic selected_workers when ample peers exist: {agents:?}"
    );
}

#[test]
fn ollama_api_only_availability_ranks_as_full_weekly_headroom() {
    let config = config_from_json(
        r#"{
          "version": 1,
          "mainProviders": ["ollama-glm", "grok"],
          "providers": [
            {
              "id": "ollama-glm",
              "agent": "claudex-ollama-glm-5-2",
              "defaultModel": "glm-5.2:cloud",
              "effort": "max",
              "enabled": true,
              "usageProvider": "ollama",
              "modelPrefixes": ["glm-"],
              "backend": "configured-acp"
            },
            {
              "id": "grok",
              "agent": "claudex-grok",
              "defaultModel": "grok-4.5",
              "effort": "high",
              "enabled": true,
              "usageProvider": "grok",
              "modelPrefixes": ["grok"],
              "backend": "grok-acp"
            }
          ],
          "fallback": {
            "agent": "claudex-sonnet",
            "model": "claude-sonnet-5",
            "effort": "high"
          },
          "nativeWorkers": [],
          "advisor": {
            "agent": "custom-advisor",
            "model": "claude-fable-5",
            "effort": "xhigh"
          }
        }"#,
    );
    let usage = json!([
        {
          "provider": "ollama",
          "available": true,
          "maxUsedPercent": 0,
          "reason": "available-ollama-api-only"
        },
        {
          "provider": "grok",
          "available": true,
          "reason": "available",
          "maxUsedPercent": 81,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":19},
            {"name":"seven-day","remainingPercent":19}
          ]
        }
    ]);
    let summary = routing_summary(&usage, &config, &BTreeSet::new()).unwrap();
    assert_eq!(summary["selected_workers"][0]["agent"], "claudex-ollama-glm-5-2");
    let agents = selected_agents(&summary);
    assert!(
        !agents.contains(&"claudex-grok"),
        "depleted grok should not stay selected beside ample ollama: {agents:?}"
    );
}

#[test]
fn suppresses_sonnet_when_main_is_sonnet() {
    let summary = routing_summary(&json!([]), &sample_config(), &BTreeSet::new()).unwrap();
    let separated =
        enforce_worker_model_separation(summary, Some("claude-sonnet-5"), true, false).unwrap();
    assert!(separated["sonnet_subagent_suppressed"].as_bool().unwrap());
    assert!(separated["selected_workers"].as_array().unwrap().is_empty());
    assert_eq!(separated["direct_main_execution"], "allowed");
}

#[test]
fn disabled_models_are_excluded() {
    let disabled = BTreeSet::from(["gpt-5.3-codex-spark".to_owned()]);
    let summary = routing_summary(&report(), &sample_config(), &disabled).unwrap();
    assert!(!selected_models(&summary).contains(&"gpt-5.3-codex-spark"));
    assert_eq!(
        summary["providers"]["codex"]["reason"],
        "disabled-by-policy"
    );
}

#[test]
fn task_fanout_is_bounded() {
    assert_eq!(task_fanout(0, 5, None).unwrap(), 0);
    assert_eq!(task_fanout(1, 5, None).unwrap(), 1);
    assert_eq!(task_fanout(8, 5, None).unwrap(), 5);
}

#[test]
fn multi_scope_fanout_exceeds_one_when_capacity_allows() {
    let summary = routing_summary(&report(), &sample_config(), &BTreeSet::new()).unwrap();
    let workers = summary["selected_workers"].as_array().unwrap().len() as i64;
    assert!(workers >= 2, "fixture must expose multiple workers");
    assert_eq!(task_fanout(1, workers, Some(&summary)).unwrap(), 1);
    assert!(task_fanout(3, workers, Some(&summary)).unwrap() >= 2);
    assert!(task_fanout(5, workers, Some(&summary)).unwrap() >= 2);
    let orch = summary["orchestration"].as_object().unwrap();
    assert_eq!(orch["task_fanout_default"], 1);
    assert_eq!(orch["single_scope_fanout"], 1);
    let multi = orch["multi_scope_example_fanout"].as_i64().unwrap();
    assert!(
        multi >= 2,
        "multi_scope_example_fanout must be >1 when workers exist, got {multi}"
    );
    let examples = orch["task_fanout_examples"].as_array().unwrap();
    let three = examples
        .iter()
        .find(|entry| entry["independent_scopes"] == 3)
        .unwrap();
    assert!(three["fanout"].as_i64().unwrap() >= 2);
    assert!(
        orch["minimum_subagents_per_phase"].as_i64().unwrap() >= 2,
        "phase minimum must not stay hard-coded at 1"
    );
    assert!(orch["minimum_active_subagents"].as_i64().unwrap() >= 2);
    assert!(orch["minimum_model_kinds"].as_i64().unwrap() >= 2);
}

#[test]
fn default_subagent_route_names_top_worker() {
    let summary = routing_summary(&report(), &sample_config(), &BTreeSet::new()).unwrap();
    let route = default_subagent_route(&summary).unwrap();
    assert_eq!(route["model"], summary["selected_workers"][0]["model"]);
    assert_eq!(route["applies_to_subagent_types"][0], "general-purpose");
    assert_eq!(route["applies_when_claudex_model_omitted"], true);
}

#[test]
fn pressure_bands_map_to_caps() {
    let thresholds = (10.0, 20.0, 30.0, 40.0);
    assert_eq!(pressure_level(5.0, thresholds), "critical");
    assert_eq!(memory_parallel_cap(5.0, thresholds), Some(2));
    assert_eq!(memory_parallel_cap(15.0, thresholds), Some(6));
    assert_eq!(memory_parallel_cap(25.0, thresholds), Some(16));
    assert_eq!(memory_parallel_cap(35.0, thresholds), Some(32));
    assert_eq!(memory_parallel_cap(50.0, thresholds), None);
}

#[test]
fn claude_windows_normalize() {
    let entry = json!({
        "provider":"claude",
        "usage":{
          "primary":{"usedPercent":12.5,"resetsAt":"2099-01-01T00:00:00Z"},
          "secondary":{"usedPercent":40.0,"resetsAt":"2099-01-01T00:00:00Z"}
        }
    });
    let normalized = codexbar_window_quota_entry(Some(&entry), None).unwrap();
    assert_eq!(normalized["reason"], "available-claude-quota");
    assert_eq!(normalized["quotaWindows"][0]["name"], "five-hour");
    assert_eq!(normalized["quotaWindows"][1]["name"], "seven-day");
}

#[test]
fn spark_weekly_extra_window_replaces_secondary() {
    let config = spark_weekly_config();
    let summary = routing_summary(&spark_report(100.0), &config, &BTreeSet::new()).unwrap();
    assert_eq!(summary["providers"]["codex"]["available"], true);
    assert_eq!(summary["providers"]["codex-spark"]["available"], false);
    assert_eq!(summary["providers"]["codex-spark"]["reason"], "exhausted");
    let agents = selected_agents(&summary);
    assert!(agents.contains(&"claudex-gpt"));
    assert!(!agents.contains(&"claudex-gpt-spark"));

    let open = routing_summary(&spark_report(20.0), &config, &BTreeSet::new()).unwrap();
    assert_eq!(open["providers"]["codex-spark"]["available"], true);
    assert_eq!(
        open["providers"]["codex-spark"]["quota_windows"]["seven-day"],
        80.0
    );
    assert_eq!(
        open["providers"]["codex"]["quota_windows"]["seven-day"],
        14.0
    );
}

#[test]
fn spark_weekly_window_missing_fails_closed() {
    let missing = routing_summary(
        &json!([{
          "provider":"codex",
          "usage":{"primary":{"usedPercent":10.0},"secondary":{"usedPercent":86.0}}
        }]),
        &spark_weekly_config(),
        &BTreeSet::new(),
    )
    .unwrap();
    assert_eq!(missing["providers"]["codex-spark"]["available"], false);
    assert_eq!(
        missing["providers"]["codex-spark"]["reason"],
        "usage-weekly-window-missing"
    );
}

#[test]
fn sonnet_native_worker_shares_claude_usage_left_with_haiku() {
    let usage = json!([
        {
          "provider":"codex",
          "usage":{"primary":{"usedPercent":50},"secondary":{"usedPercent":60}}
        },
        {
          "provider":"claude",
          "usage":{
            "primary":{"usedPercent":20.0},
            "secondary":{"usedPercent":35.0}
          }
        }
    ]);
    let summary = routing_summary(&usage, &native_claude_config(), &BTreeSet::new()).unwrap();
    let agents: Vec<_> = summary["selected_workers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|worker| worker.get("agent").and_then(Value::as_str))
        .collect();
    assert!(agents.contains(&"claudex-haiku-search"));
    assert!(agents.contains(&"claudex-sonnet"));
    assert_eq!(
        summary["native_worker_quota"]["claudex-haiku-search"]["max_used_percent"],
        summary["native_worker_quota"]["claudex-sonnet"]["max_used_percent"]
    );
    assert_eq!(
        summary["native_worker_quota"]["claudex-sonnet"]["max_used_percent"],
        35.0
    );
    let capacity = worker_capacity_metadata(&summary);
    let haiku = capacity
        .iter()
        .find(|entry| entry["agent"] == "claudex-haiku-search")
        .unwrap();
    let sonnet = capacity
        .iter()
        .find(|entry| entry["agent"] == "claudex-sonnet")
        .unwrap();
    assert_eq!(haiku["usage_provider"], "claude");
    assert_eq!(sonnet["usage_provider"], "claude");
    assert_eq!(haiku["remaining_percent"], sonnet["remaining_percent"]);
    assert_eq!(haiku["weekly_remaining_percent"], 65.0);
    assert_eq!(sonnet["weekly_remaining_percent"], 65.0);
    assert_eq!(haiku["five_hour_remaining_percent"], 80.0);
    assert_eq!(sonnet["five_hour_remaining_percent"], 80.0);
}

#[test]
fn qwencloud_windows_normalize_like_claude() {
    let entry = json!({
        "provider":"qwencloud",
        "usage":{
          "primary":{"usedPercent":0.0,"windowMinutes":300},
          "secondary":{"usedPercent":50.045935172323,"windowMinutes":10080,"resetsAt":"2099-01-01T00:00:00Z"}
        }
    });
    let normalized = codexbar_window_quota_entry(Some(&entry), None).unwrap();
    assert_eq!(normalized["reason"], "available-qwencloud-quota");
    assert_eq!(normalized["quotaWindows"][0]["name"], "five-hour");
    assert_eq!(normalized["quotaWindows"][1]["name"], "seven-day");
    assert!(normalized["maxUsedPercent"].as_f64().unwrap() > 50.0);
}

#[test]
fn concurrency_refresh_preserves_selection_without_daemon_health() {
    let summary = routing_summary(&report(), &sample_config(), &BTreeSet::new()).unwrap();
    let before = selected_agents(&summary).len();
    let refreshed =
        apply_model_concurrency(summary, &sample_config(), None, &BTreeSet::new()).unwrap();
    assert_eq!(selected_agents(&refreshed).len(), before);
    assert!(refreshed["model_concurrency"].is_object());
}
