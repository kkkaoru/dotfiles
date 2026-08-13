use super::concurrency::apply_model_concurrency_with_inflight;
use super::orchestration::task_fanout;
use super::quota::codexbar_window_quota_entry;
use super::summary::{fallback_summary, routing_summary, routing_summary_with_exhaustion};
use super::workers::{
    default_subagent_route, enforce_worker_model_separation, worker_capacity_metadata,
};
use super::{
    CUSTOM_ADVISOR_CONSULT_WHEN, DEFAULT_ACTIVE_SUBAGENT_FLOOR, DEFAULT_MIN_MODEL_KINDS,
    DEFAULT_MIN_SUBAGENTS_PER_PHASE, memory_parallel_cap, pressure_level,
};
use crate::config::{Config, load_config};
use serde_json::{Value, json};
use std::collections::BTreeMap;
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
              "defaultModel": "grok-4.6",
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

fn selected_agent_model_count(summary: &Value, agent: &str, model: &str) -> usize {
    summary["selected_workers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|worker| {
            worker.get("agent").and_then(Value::as_str) == Some(agent)
                && worker.get("model").and_then(Value::as_str) == Some(model)
        })
        .count()
}

#[test]
fn command_code_is_auto_selected_beside_metered_peers() {
    // Historical bug: unmetered command-code was dropped whenever any peer had
    // >=40% weekly remaining, so orchestrators never auto-launched Muse Spark.
    let config = config_from_json(
        r#"{
          "version": 1,
          "mainProviders": ["codex", "command-code"],
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
              "id": "command-code",
              "agent": "claudex-command-code-muse-spark-1-2-contributor",
              "defaultModel": "meta/muse-spark-1.2-contributor",
              "effort": "high",
              "enabled": true,
              "modelPrefixes": ["meta/muse-spark"],
              "backend": "configured-acp",
              "acp": {"program":"command-code-acp","arguments":["--model","{model}"]}
            }
          ],
          "fallback": {
            "agent": "claudex-sonnet",
            "model": "claude-sonnet-5",
            "effort": "high"
          },
          "nativeWorkers": []
        }"#,
    );
    let summary = routing_summary(&report(), &config, &BTreeSet::new()).unwrap();
    let agents = selected_agents(&summary);
    assert_eq!(agents[0], "claudex-gpt-spark");
    assert!(
        agents.iter().any(|agent| agent.contains("command-code")),
        "unmetered command-code must stay in automatic selected_workers: {agents:?}"
    );
}

#[test]
fn command_code_uses_codexbar_commandcode_quota() {
    // Live CodexBar provider id is `commandcode`. Without usageProvider the
    // worker stayed unmetered even though weekly/five-hour left was available.
    let config = config_from_json(
        r#"{
          "version": 1,
          "mainProviders": ["codex", "command-code"],
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
              "id": "command-code",
              "agent": "claudex-command-code-muse-spark-1-2-contributor",
              "defaultModel": "meta/muse-spark-1.2-contributor",
              "effort": "high",
              "enabled": true,
              "usageProvider": "commandcode",
              "modelPrefixes": ["meta/muse-spark"],
              "backend": "configured-acp",
              "acp": {"program":"command-code-acp","arguments":["--model","{model}"]}
            }
          ],
          "fallback": {
            "agent": "claudex-sonnet",
            "model": "claude-sonnet-5",
            "effort": "high"
          },
          "nativeWorkers": []
        }"#,
    );
    let usage = json!([
        {
          "provider": "codex",
          "usage": {"primary": {"usedPercent": 2.0}, "secondary": {"usedPercent": 2.0}}
        },
        {
          "provider": "commandcode",
          "usage": {
            "primary": {"usedPercent": 7.4, "windowMinutes": 300},
            "secondary": {"usedPercent": 10.9, "windowMinutes": 10080}
          }
        }
    ]);
    let summary = routing_summary(&usage, &config, &BTreeSet::new()).unwrap();
    let command = &summary["providers"]["command-code"];
    assert_eq!(command["available"], true);
    assert_eq!(command["reason"], "available-commandcode-quota");
    assert_eq!(command["quota_windows"]["five-hour"], 92.6);
    assert_eq!(command["quota_windows"]["seven-day"], 89.1);
    let agents = selected_agents(&summary);
    assert_eq!(agents[0], "claudex-gpt");
    assert!(
        agents.iter().any(|agent| agent.contains("command-code")),
        "metered command-code must stay in automatic selected_workers: {agents:?}"
    );
    let worker = summary["selected_workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item.get("agent")
                .and_then(Value::as_str)
                .is_some_and(|agent| agent.contains("command-code"))
        })
        .expect("command-code worker");
    assert_eq!(worker["weekly_remaining_percent"], 89.1);
    assert_eq!(worker["five_hour_remaining_percent"], 92.6);
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
    assert_eq!(
        summary["orchestration"]["fanout_matches_independent_scopes"],
        true
    );
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
            {"name":"five-hour","remainingPercent":30},
            {"name":"seven-day","remainingPercent":80}
          ]
        },
        {"provider":"qwencloud","available":false,"reason":"exhausted","maxUsedPercent":100}
    ]);
    let summary = routing_summary(&usage, &sample_config(), &BTreeSet::new()).unwrap();
    // Ranking prefers weekly remaining: grok=80, spark=40.
    assert_eq!(summary["selected_workers"][0]["agent"], "claudex-grok");
    assert_eq!(summary["selected_workers"][1]["agent"], "claudex-gpt-spark");
}

#[test]
fn five_hour_bottleneck_does_not_outrank_fatter_weekly() {
    let usage = json!([
        {
          "provider":"codex",
          "available": true,
          "reason": "available",
          "maxUsedPercent": 50,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":30},
            {"name":"seven-day","remainingPercent":95}
          ]
        },
        {
          "provider":"grok",
          "available": true,
          "reason": "available",
          "maxUsedPercent": 40,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":70},
            {"name":"seven-day","remainingPercent":55}
          ]
        },
        {"provider":"qwencloud","available":false,"reason":"exhausted","maxUsedPercent":100}
    ]);
    let summary = routing_summary(&usage, &sample_config(), &BTreeSet::new()).unwrap();
    // Weekly order wins for ranking; five-hour only filters via prefer_weekly_headroom.
    assert_eq!(summary["selected_workers"][0]["agent"], "claudex-gpt-spark");
    assert_eq!(summary["selected_workers"][1]["agent"], "claudex-grok");
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
fn drops_low_five_hour_workers_when_ample_alternatives_exist() {
    let usage = json!([
        {
          "provider":"codex",
          "available": true,
          "reason": "available",
          "maxUsedPercent": 20,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":10},
            {"name":"seven-day","remainingPercent":90}
          ]
        },
        {
          "provider":"grok",
          "available": true,
          "reason": "available",
          "maxUsedPercent": 30,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":80},
            {"name":"seven-day","remainingPercent":50}
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
    assert!(agents.contains(&"claudex-grok"));
    assert!(
        !agents.contains(&"claudex-gpt-spark"),
        "high weekly / low five-hour spark must leave automatic selected_workers: {agents:?}"
    );
}

const EXHAUSTION_COOLDOWN_USAGE: &str = r#"[
        {
          "provider":"codex", "available":true, "reason":"available", "maxUsedPercent":20,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":80},
            {"name":"seven-day","remainingPercent":80}
          ]
        },
        {
          "provider":"grok", "available":true, "reason":"available", "maxUsedPercent":10,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":90},
            {"name":"seven-day","remainingPercent":90}
          ]
        },
        {
          "provider":"qwencloud", "available":true, "reason":"available", "maxUsedPercent":5,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":95},
            {"name":"seven-day","remainingPercent":95}
          ]
        },
        {
          "provider":"ollama", "available":true, "reason":"available", "maxUsedPercent":1,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":99},
            {"name":"seven-day","remainingPercent":99}
          ]
        }
    ]"#;

const EXHAUSTION_COOLDOWN_CONFIG: &str = r#"{
          "version": 1,
          "mainProviders": ["codex", "grok", "ollama-glm"],
          "providers": [
            {
              "id": "codex",
              "agent": "claudex-gpt-spark",
              "defaultModel": "gpt-5.3-codex-spark",
              "effort": "high",
              "enabled": true,
              "usageProvider": "codex",
              "backend": "codex-app-server"
            },
            {
              "id": "grok",
              "agent": "claudex-grok",
              "defaultModel": "grok-4.6",
              "effort": "high",
              "enabled": true,
              "usageProvider": "grok",
              "backend": "grok-acp"
            },
            {
              "id": "ollama-glm",
              "agent": "claudex-ollama-glm-5-2",
              "defaultModel": "glm-5.2:cloud",
              "effort": "max",
              "enabled": true,
              "usageProvider": "ollama",
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
        }"#;

#[test]
fn drops_exhausted_cooldown_providers_from_automatic_selection() {
    let usage: Value = serde_json::from_str(EXHAUSTION_COOLDOWN_USAGE).unwrap();
    let config = config_from_json(EXHAUSTION_COOLDOWN_CONFIG);
    let scopes = BTreeSet::from(["ollama".to_owned()]);
    let summary =
        routing_summary_with_exhaustion(&usage, &config, &BTreeSet::new(), &scopes, false).unwrap();
    let agents = selected_agents(&summary);
    assert!(
        !agents.contains(&"claudex-ollama-glm-5-2"),
        "cooldown ollama must leave automatic selected_workers: {agents:?}"
    );
    assert_eq!(
        summary["providers"]["ollama-glm"]["reason"],
        "provider-exhaustion-cooldown"
    );
    let disabled = summary["disabled_subagent_models"]
        .as_array()
        .expect("disabled models");
    assert!(
        disabled
            .iter()
            .any(|model| model.as_str() == Some("glm-5.2:cloud")),
        "cooldown ollama must denylist glm-5.2:cloud: {disabled:?}"
    );
}

#[test]
fn drops_codexbar_weekly_limit_ollama_and_disables_glm() {
    let usage = json!([{
        "provider": "codex",
        "usage": {
            "primary": {"usedPercent": 10.0},
            "secondary": {"usedPercent": 20.0}
        }
    }, {
        "provider": "ollama",
        "usage": {
            "primary": {"usedPercent": 0.0, "resetsAt": "2026-08-10T00:00:00Z"},
            "secondary": {"usedPercent": 100.0, "resetsAt": "2026-08-10T00:00:00Z"}
        }
    }]);
    let config = config_from_json(
        r#"{
          "version": 1,
          "mainProviders": ["codex", "ollama-glm"],
          "providers": [
            {
              "id": "codex",
              "agent": "claudex-gpt-spark",
              "defaultModel": "gpt-5.3-codex-spark",
              "effort": "high",
              "enabled": true,
              "usageProvider": "codex",
              "backend": "codex-app-server"
            },
            {
              "id": "ollama-glm",
              "agent": "claudex-ollama-glm-5-2",
              "defaultModel": "glm-5.2:cloud",
              "effort": "max",
              "enabled": true,
              "usageProvider": "ollama",
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
    );
    let summary = routing_summary(&usage, &config, &BTreeSet::new()).unwrap();
    let agents = selected_agents(&summary);
    assert!(
        !agents.contains(&"claudex-ollama-glm-5-2"),
        "CodexBar weekly 100% ollama must leave automatic selected_workers: {agents:?}"
    );
    assert_eq!(summary["providers"]["ollama-glm"]["reason"], "exhausted");
    let disabled = summary["disabled_subagent_models"]
        .as_array()
        .expect("disabled models");
    assert!(
        disabled
            .iter()
            .any(|model| model.as_str() == Some("glm-5.2:cloud")),
        "CodexBar weekly limit must denylist glm-5.2:cloud: {disabled:?}"
    );
}

#[test]
fn ollama_api_only_availability_ranks_behind_known_weekly() {
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
              "defaultModel": "grok-4.6",
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
    // API reachability must not invent weekly headroom over a real meter.
    let usage = json!([
        {
          "provider": "ollama",
          "available": true,
          "reason": "available-ollama-api-only"
        },
        {
          "provider": "grok",
          "available": true,
          "reason": "available",
          "maxUsedPercent": 23,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":100},
            {"name":"seven-day","remainingPercent":77}
          ]
        }
    ]);
    let summary = routing_summary(&usage, &config, &BTreeSet::new()).unwrap();
    assert_eq!(summary["selected_workers"][0]["agent"], "claudex-grok");
    assert_eq!(
        summary["selected_workers"][0]["weekly_remaining_percent"],
        77.0
    );
    let agents = selected_agents(&summary);
    assert!(
        !agents.contains(&"claudex-ollama-glm-5-2"),
        "api-only ollama must leave automatic selected_workers when metered peers have ample headroom: {agents:?}"
    );
}

#[test]
fn ollama_api_only_is_usable_when_no_weekly_meters_exist() {
    let config = config_from_json(
        r#"{
          "version": 1,
          "mainProviders": ["ollama-glm"],
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
    let usage = json!([{
      "provider": "ollama",
      "available": true,
      "reason": "available-ollama-api-only"
    }]);
    let summary = routing_summary(&usage, &config, &BTreeSet::new()).unwrap();
    assert_eq!(
        summary["selected_workers"][0]["agent"],
        "claudex-ollama-glm-5-2"
    );
    assert!(summary["selected_workers"][0]["weekly_remaining_percent"].is_null());
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
fn disabled_models_remain_visible_only_in_policy_metadata() {
    let disabled = BTreeSet::from(["gpt-5.3-codex-spark".to_owned()]);
    let summary = routing_summary(&report(), &sample_config(), &disabled).unwrap();
    assert_ne!(selected_models(&summary)[0], "gpt-5.3-codex-spark");
    assert!(!selected_models(&summary).contains(&"gpt-5.3-codex-spark"));
    assert_eq!(
        summary["providers"]["codex"]["reason"],
        "disabled-by-policy"
    );
    assert!(
        summary["disabled_subagent_models"]
            .as_array()
            .expect("disabled models")
            .iter()
            .any(|model| model.as_str() == Some("gpt-5.3-codex-spark"))
    );
}

#[test]
fn disabled_models_are_not_visible_in_selected_workers() {
    let config = config_from_json(
        r#"{
          "version": 1,
          "mainProviders": ["codex", "opencode"],
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
              "id": "opencode",
              "agent": "claudex-deepseek-flash",
              "defaultModel": "opencode-go/deepseek-v4-flash",
              "effort": "high",
              "enabled": true,
              "usageProvider": "opencodego",
              "modelPrefixes": ["opencode-go/"],
              "backend": "configured-acp"
            }
          ],
          "fallback": {
            "agent": "claudex-sonnet",
            "model": "claude-sonnet-5",
            "effort": "high"
          },
          "nativeWorkers": []
        }"#,
    );
    let usage = json!([
        {
          "provider":"codex",
          "usage": {
            "primary":{"usedPercent":10.0},
            "secondary":{"usedPercent":20.0}
          }
        },
        {
          "provider":"opencodego",
          "usage": {
            "primary":{"usedPercent":2.0},
            "secondary":{"usedPercent":90.0}
          }
        }
    ]);
    let disabled = BTreeSet::from(["opencode-go/deepseek-v4-flash".to_owned()]);
    let summary = routing_summary(&usage, &config, &disabled).unwrap();
    let workers = selected_models(&summary);
    assert_eq!(workers[0], "gpt-5.6-luna");
    assert!(!workers.contains(&"opencode-go/deepseek-v4-flash"));
    assert_eq!(summary["providers"]["opencode"]["disabled"], true);
    assert!(
        summary["disabled_subagent_models"]
            .as_array()
            .expect("disabled models")
            .iter()
            .any(|model| model.as_str() == Some("opencode-go/deepseek-v4-flash"))
    );
}

#[test]
fn hostname_denylist_overrides_providers_json_enabled_in_selected_workers() {
    let config = config_from_json(
        r#"{
          "version": 1,
          "mainProviders": ["codex", "opencode"],
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
              "id": "opencode",
              "agent": "claudex-deepseek-flash",
              "defaultModel": "opencode-go/deepseek-v4-flash",
              "effort": "high",
              "enabled": true,
              "usageProvider": "opencodego",
              "modelPrefixes": ["opencode-go/"],
              "backend": "configured-acp"
            }
          ],
          "fallback": {
            "agent": "claudex-sonnet",
            "model": "claude-sonnet-5",
            "effort": "high"
          },
          "nativeWorkers": []
        }"#,
    );
    let denylist = BTreeSet::from(["opencode-go/deepseek-v4-flash".to_owned()]);
    let raw_providers = json!([
        {"id":"codex","defaultModel":"gpt-5.6-luna","enabled":true},
        {"id":"opencode","defaultModel":"opencode-go/deepseek-v4-flash","enabled":true}
    ]);
    assert_eq!(
        crate::config::enabled_denylist_conflicts(raw_providers.as_array().unwrap(), &denylist),
        ["opencode".to_owned()]
    );
    let usage = json!([
        {"provider":"codex","usage":{"primary":{"usedPercent":10.0},"secondary":{"usedPercent":20.0}}},
        {"provider":"opencodego","usage":{"primary":{"usedPercent":2.0},"secondary":{"usedPercent":5.0}}}
    ]);
    let summary = routing_summary(&usage, &config, &denylist).unwrap();
    let workers = selected_models(&summary);
    assert!(!workers.contains(&"opencode-go/deepseek-v4-flash"));
    assert_eq!(summary["providers"]["opencode"]["disabled"], true);
}

#[test]
fn task_fanout_is_bounded() {
    assert_eq!(task_fanout(0, 5, None).unwrap(), 0);
    assert_eq!(task_fanout(1, 5, None).unwrap(), 1);
    assert_eq!(task_fanout(8, 5, None).unwrap(), 5);
}

#[test]
fn default_floors_do_not_force_three_workers_on_one_scope() {
    assert_eq!(DEFAULT_MIN_SUBAGENTS_PER_PHASE, 1);
    assert_eq!(DEFAULT_ACTIVE_SUBAGENT_FLOOR, 1);
    assert_eq!(DEFAULT_MIN_MODEL_KINDS, 1);
    assert_eq!(task_fanout(1, 40, None).unwrap(), 1);
}

#[test]
fn custom_advisor_consult_when_is_conflict_and_high_risk_only() {
    assert!(!CUSTOM_ADVISOR_CONSULT_WHEN.contains(&"external_research_or_multiple_sources"));
    assert!(!CUSTOM_ADVISOR_CONSULT_WHEN.contains(&"complex_or_ambiguous_decision"));
    assert!(!CUSTOM_ADVISOR_CONSULT_WHEN.contains(&"long_running_phase_over_ten_minutes"));
    assert!(CUSTOM_ADVISOR_CONSULT_WHEN.contains(&"conflicting_worker_results"));
    assert!(CUSTOM_ADVISOR_CONSULT_WHEN.contains(&"high_risk_implementation_or_config_change"));
    assert!(CUSTOM_ADVISOR_CONSULT_WHEN.contains(&"worker_failure_timeout_or_stall"));
}

#[test]
fn multi_scope_fanout_exceeds_one_when_capacity_allows() {
    let summary = routing_summary(&report(), &sample_config(), &BTreeSet::new()).unwrap();
    let workers = summary["selected_workers"].as_array().unwrap().len() as i64;
    assert!(workers >= 2, "fixture must expose multiple workers");
    assert_eq!(task_fanout(1, workers, Some(&summary)).unwrap(), 1);
    assert_eq!(
        task_fanout(3, workers, Some(&summary)).unwrap(),
        3.min(workers)
    );
    assert_eq!(
        task_fanout(5, workers, Some(&summary)).unwrap(),
        5.min(workers)
    );
    let orch = summary["orchestration"].as_object().unwrap();
    assert_eq!(orch["task_fanout_default"], 1);
    assert_eq!(orch["single_scope_fanout"], 1);
    assert_eq!(orch["fanout_matches_independent_scopes"], true);
    let multi = orch["multi_scope_example_fanout"].as_i64().unwrap();
    assert!(
        multi >= 2,
        "multi_scope_example_fanout must be >1 when workers exist, got {multi}"
    );
    let examples = orch["task_fanout_examples"].as_array().unwrap();
    let one = examples
        .iter()
        .find(|entry| entry["independent_scopes"] == 1)
        .unwrap();
    assert_eq!(one["fanout"].as_i64().unwrap(), 1);
    let three = examples
        .iter()
        .find(|entry| entry["independent_scopes"] == 3)
        .unwrap();
    assert_eq!(three["fanout"].as_i64().unwrap(), 3.min(workers));
    assert!(
        !orch["custom_advisor_consult_when"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str() == Some("external_research_or_multiple_sources"))
    );
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
fn default_route_skips_policy_disabled_models() {
    let config = config_from_json(
        r#"{
          "version": 1,
          "mainProviders": ["codex", "opencode"],
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
              "id": "opencode",
              "agent": "claudex-deepseek-flash",
              "defaultModel": "opencode-go/deepseek-v4-flash",
              "effort": "high",
              "enabled": true,
              "usageProvider": "opencodego",
              "modelPrefixes": ["opencode-go/"],
              "backend": "configured-acp"
            }
          ],
          "fallback": {
            "agent": "claudex-sonnet",
            "model": "claude-sonnet-5",
            "effort": "high"
          },
          "nativeWorkers": []
        }"#,
    );
    let usage = json!([
        {
          "provider":"codex",
          "usage": {
            "primary":{"usedPercent":10.0},
            "secondary":{"usedPercent":20.0}
          }
        },
        {
          "provider":"opencodego",
          "usage": {
            "primary":{"usedPercent":2.0},
            "secondary":{"usedPercent":90.0}
          }
        }
    ]);
    let disabled = BTreeSet::from(["opencode-go/deepseek-v4-flash".to_owned()]);
    let summary = routing_summary(&usage, &config, &disabled).unwrap();
    let selected = selected_models(&summary);
    let route = default_subagent_route(&summary).unwrap();

    assert!(!selected.contains(&"opencode-go/deepseek-v4-flash"));
    assert_eq!(route["model"], summary["selected_workers"][0]["model"]);
    assert_ne!(route["model"], "opencode-go/deepseek-v4-flash");
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
fn fallback_summary_deduplicates_matching_native_worker() {
    let summary = fallback_summary(
        "usage-snapshot-missing",
        &native_claude_config(),
        &BTreeSet::new(),
    )
    .unwrap();
    let agents = selected_agents(&summary);

    assert_eq!(
        selected_agent_model_count(&summary, "claudex-sonnet", "claude-sonnet-5"),
        1
    );
    assert_eq!(
        summary["selected_workers"][0]["provider"], "fallback",
        "the explicit fallback must win over its matching native worker"
    );
    assert!(agents.contains(&"claudex-haiku-search"));
    assert_eq!(agents.len(), agents.iter().collect::<BTreeSet<_>>().len());
    assert_eq!(
        summary["orchestration"]["max_available_workers"].as_u64(),
        Some(agents.len() as u64),
        "fanout capacity must not count a duplicate fallback/native worker"
    );
    assert_eq!(summary["fallback_active"], true);
}

#[test]
fn concurrency_refresh_does_not_reintroduce_fallback_native_duplicate() {
    let config = native_claude_config();
    let summary = fallback_summary("usage-snapshot-missing", &config, &BTreeSet::new()).unwrap();
    let refreshed = apply_model_concurrency_with_inflight(
        summary,
        &config,
        None,
        &BTreeMap::new(),
        &BTreeSet::new(),
    )
    .unwrap();

    assert_eq!(
        selected_agent_model_count(&refreshed, "claudex-sonnet", "claude-sonnet-5"),
        1
    );
    assert_eq!(refreshed["selected_workers"][0]["provider"], "fallback");
    assert!(selected_agents(&refreshed).contains(&"claudex-haiku-search"));
}

#[test]
fn disabled_fallback_is_absent_while_distinct_native_worker_remains() {
    let config = native_claude_config();
    let disabled = BTreeSet::from(["claude-sonnet-5".to_owned()]);
    let summary = fallback_summary("usage-snapshot-missing", &config, &disabled).unwrap();
    let workers = summary["selected_workers"].as_array().unwrap();

    assert_eq!(summary["fallback_active"], false);
    assert!(
        workers
            .iter()
            .all(|worker| worker["model"] != "claude-sonnet-5")
    );
    assert_eq!(workers.len(), 1);
    assert_eq!(workers[0]["agent"], "claudex-haiku-search");
    assert_eq!(summary["preferred_worker"], workers[0]);
    assert_eq!(summary["selected_agents"], json!(["claudex-haiku-search"]));
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
    let refreshed = apply_model_concurrency_with_inflight(
        summary,
        &sample_config(),
        None,
        &BTreeMap::new(),
        &BTreeSet::new(),
    )
    .unwrap();
    assert_eq!(selected_agents(&refreshed).len(), before);
    assert!(refreshed["model_concurrency"].is_object());
}

#[test]
fn inflight_subagents_demote_busy_models_down_the_weekly_order() {
    let usage = json!([
        {
          "provider":"codex",
          "available": true,
          "reason": "available",
          "maxUsedPercent": 10,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":90},
            {"name":"seven-day","remainingPercent":90}
          ]
        },
        {
          "provider":"grok",
          "available": true,
          "reason": "available",
          "maxUsedPercent": 40,
          "quotaWindows": [
            {"name":"five-hour","remainingPercent":70},
            {"name":"seven-day","remainingPercent":60}
          ]
        },
        {"provider":"qwencloud","available":false,"reason":"exhausted","maxUsedPercent":100}
    ]);
    let summary = routing_summary(&usage, &sample_config(), &BTreeSet::new()).unwrap();
    assert_eq!(summary["selected_workers"][0]["agent"], "claudex-gpt-spark");
    let mut inflight = BTreeMap::new();
    inflight.insert("gpt-5.3-codex-spark".to_owned(), 1);
    let refreshed = apply_model_concurrency_with_inflight(
        summary,
        &sample_config(),
        None,
        &inflight,
        &BTreeSet::new(),
    )
    .unwrap();
    assert_eq!(
        refreshed["selected_workers"][0]["agent"], "claudex-grok",
        "busy top weekly worker must yield to the next weekly-ranked peer: {}",
        refreshed["selected_workers"]
    );
}

#[test]
fn concurrency_refresh_keeps_policy_disabled_models_out_of_ranking() {
    let config = config_from_json(
        r#"{
          "version": 1,
          "mainProviders": ["codex", "opencode"],
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
              "id": "opencode",
              "agent": "claudex-deepseek-flash",
              "defaultModel": "opencode-go/deepseek-v4-flash",
              "effort": "high",
              "enabled": true,
              "usageProvider": "opencodego",
              "modelPrefixes": ["opencode-go/"],
              "backend": "configured-acp",
              "maxConcurrency": 8,
              "acp": {"program": "opencode", "arguments": ["--model", "{model}"]}
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
          "provider":"codex",
          "usage": {
            "primary":{"usedPercent":10.0},
            "secondary":{"usedPercent":20.0}
          }
        },
        {
          "provider":"opencodego",
          "usage": {
            "primary":{"usedPercent":2.0},
            "secondary":{"usedPercent":90.0}
          }
        }
    ]);
    let disabled = BTreeSet::from(["opencode-go/deepseek-v4-flash".to_owned()]);
    let summary = routing_summary(&usage, &config, &disabled).unwrap();
    let refreshed =
        apply_model_concurrency_with_inflight(summary, &config, None, &BTreeMap::new(), &disabled)
            .unwrap();
    let workers = selected_models(&refreshed);
    assert_eq!(workers[0], "gpt-5.6-luna");
    assert!(!workers.contains(&"opencode-go/deepseek-v4-flash"));
}
