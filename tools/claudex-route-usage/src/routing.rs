//! Capacity ranking, worker selection, concurrency refresh, and orchestration.

use crate::config::Config;
use crate::opencode_go_budget;
use crate::util::{
    boolean_env, is_sonnet_model, model_family, number_f64, positive_or_default, python_round,
    valid_percentage,
};
use anyhow::Result;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::env;

pub const FIVE_HOUR_WINDOW: &str = "five-hour";
pub const SEVEN_DAY_WINDOW: &str = "seven-day";
pub const DEFAULT_MAX_SUBAGENTS: i64 = 40;
pub const ORCHESTRATION_REBALANCE_INTERVAL_SECONDS: i64 = 10 * 60;
pub const DEFAULT_SUBAGENT_STATUS_POLL_SECONDS: i64 = 15;
pub const SUBAGENT_MAX_PARALLEL_ENV: &str = "CLAUDEX_SUBAGENT_MAX_PARALLEL";
pub const SUBAGENT_REEVALUATE_ON_COMPLETION_ENV: &str = "CLAUDEX_SUBAGENT_REEVALUATE_ON_COMPLETION";
pub const SUBAGENT_REASSESS_INTERVAL_ENV: &str = "CLAUDEX_SUBAGENT_REASSESS_INTERVAL_SECONDS";
pub const SUBAGENT_REUSE_ENV: &str = "CLAUDEX_SUBAGENT_REUSE";
pub const SUBAGENT_CLEANUP_ON_EXIT_ENV: &str = "CLAUDEX_SUBAGENT_CLEANUP_ON_EXIT";
pub const SUBAGENT_FIRST_ENV: &str = "CLAUDEX_SUBAGENT_FIRST";
pub const SUBAGENT_STATUS_POLL_ENV: &str = "CLAUDEX_SUBAGENT_STATUS_POLL_SECONDS";
pub const CUSTOM_ADVISOR_ENV: &str = "CLAUDEX_CUSTOM_ADVISOR";
pub const MEMORY_MANAGEMENT_ENV: &str = "CLAUDEX_MEMORY_MANAGEMENT";
pub const MEMORY_AVAILABLE_PCT_CRITICAL_ENV: &str = "CLAUDEX_MEMORY_AVAILABLE_PCT_CRITICAL";
pub const MEMORY_AVAILABLE_PCT_LOW_ENV: &str = "CLAUDEX_MEMORY_AVAILABLE_PCT_LOW";
pub const MEMORY_AVAILABLE_PCT_MEDIUM_ENV: &str = "CLAUDEX_MEMORY_AVAILABLE_PCT_MEDIUM";
pub const MEMORY_AVAILABLE_PCT_MODERATE_ENV: &str = "CLAUDEX_MEMORY_AVAILABLE_PCT_MODERATE";
pub const DEFAULT_MEMORY_AVAILABLE_PCT_CRITICAL: f64 = 10.0;
pub const DEFAULT_MEMORY_AVAILABLE_PCT_LOW: f64 = 20.0;
pub const DEFAULT_MEMORY_AVAILABLE_PCT_MEDIUM: f64 = 30.0;
pub const DEFAULT_MEMORY_AVAILABLE_PCT_MODERATE: f64 = 40.0;

pub const CUSTOM_ADVISOR_CONSULT_WHEN: &[&str] = &[
    "complex_or_ambiguous_decision",
    "external_research_or_multiple_sources",
    "high_risk_implementation_or_config_change",
    "long_running_phase_over_ten_minutes",
    "worker_failure_timeout_or_stall",
    "conflicting_worker_results",
];

type CapacityKey = (f64, f64, f64, f64, f64, f64, i64);

pub fn custom_advisor_enabled() -> bool {
    match env::var(CUSTOM_ADVISOR_ENV) {
        Ok(raw) => {
            let normalized = raw.trim().to_ascii_lowercase();
            !matches!(normalized.as_str(), "0" | "false" | "off")
        }
        Err(_) => true,
    }
}

pub fn usage_percentages(value: &Value) -> Vec<f64> {
    let mut percentages = Vec::new();
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                if key == "usedPercent" && valid_percentage(nested) {
                    if let Some(number) = number_f64(nested) {
                        percentages.push(number);
                    }
                } else {
                    percentages.extend(usage_percentages(nested));
                }
            }
        }
        Value::Array(items) => {
            for nested in items {
                percentages.extend(usage_percentages(nested));
            }
        }
        _ => {}
    }
    percentages
}

pub fn find_report_entry<'a>(report: &'a Value, provider: Option<&str>) -> Option<&'a Value> {
    let provider = provider?;
    let entries = report.as_array()?;
    entries.iter().find(|item| {
        item.as_object().is_some_and(|object| {
            object
                .get("provider")
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case(provider))
        })
    })
}

pub fn status(available: bool, maximum: Option<f64>, reason: &str) -> Value {
    serde_json::json!({
        "available": available,
        "max_used_percent": maximum,
        "remaining_percent": maximum.map(|value| (100.0 - value).max(0.0)),
        "reason": reason,
    })
}

pub fn explicitly_reported_status(entry: &Value) -> Value {
    let Some(object) = entry.as_object() else {
        return status(false, None, "unknown");
    };
    let Some(available) = object.get("available").and_then(Value::as_bool) else {
        return status(false, None, "unknown");
    };
    let reason = object
        .get("reason")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map_or_else(
            || {
                if available {
                    "available".to_owned()
                } else {
                    "usage-unavailable".to_owned()
                }
            },
            str::to_owned,
        );
    match object.get("maxUsedPercent") {
        None => status(available, None, &reason),
        Some(maximum) => {
            if !valid_percentage(maximum) || number_f64(maximum).is_some_and(|n| n > 100.0) {
                status(false, None, "unknown")
            } else {
                status(available, number_f64(maximum), &reason)
            }
        }
    }
}

pub fn provider_status(report: &Value, provider: &str) -> Value {
    let Some(entry) = find_report_entry(report, Some(provider)) else {
        return status(false, None, "missing");
    };
    if entry.get("available").is_some() {
        return explicitly_reported_status(entry);
    }
    let percentages = usage_percentages(entry.get("usage").unwrap_or(&Value::Null));
    if percentages.is_empty() {
        return status(false, None, "unknown");
    }
    let maximum = percentages.into_iter().fold(f64::NEG_INFINITY, f64::max);
    status(
        maximum < 100.0,
        Some(maximum),
        if maximum < 100.0 {
            "available"
        } else {
            "exhausted"
        },
    )
}

pub fn quota_window_remaining(entry: Option<&Value>) -> Value {
    let mut remaining = serde_json::json!({
        FIVE_HOUR_WINDOW: Value::Null,
        SEVEN_DAY_WINDOW: Value::Null,
    });
    let Some(windows) = entry
        .and_then(|value| value.get("quotaWindows"))
        .and_then(Value::as_array)
    else {
        return remaining;
    };
    for window in windows {
        let Some(object) = window.as_object() else {
            continue;
        };
        let Some(name) = object.get("name").and_then(Value::as_str) else {
            continue;
        };
        if name != FIVE_HOUR_WINDOW && name != SEVEN_DAY_WINDOW {
            continue;
        }
        let Some(value) = object.get("remainingPercent") else {
            continue;
        };
        if valid_percentage(value) && number_f64(value).is_some_and(|n| n <= 100.0) {
            remaining[name] = Value::from(number_f64(value).unwrap_or_default());
        }
    }
    remaining
}

pub fn effective_window_remaining(quota: &Value) -> (Option<f64>, Option<f64>) {
    let windows = quota.get("quota_windows");
    let weekly = windows
        .and_then(|value| value.get(SEVEN_DAY_WINDOW))
        .and_then(number_f64);
    let five_hour = windows
        .and_then(|value| value.get(FIVE_HOUR_WINDOW))
        .and_then(number_f64);
    let weekly = weekly.or_else(|| {
        quota
            .get("max_used_percent")
            .and_then(number_f64)
            .map(|maximum| (100.0 - maximum).max(0.0))
    });
    (weekly, five_hour)
}

pub fn claude_quota_entry(entry: Option<&Value>) -> Option<Value> {
    let entry = entry?;
    if !entry
        .get("provider")
        .and_then(Value::as_str)
        .is_some_and(|name| name.eq_ignore_ascii_case("claude"))
    {
        return None;
    }
    let usage = entry.get("usage")?.as_object()?;
    let mut windows = Vec::new();
    for (source, name) in [
        ("primary", FIVE_HOUR_WINDOW),
        ("secondary", SEVEN_DAY_WINDOW),
    ] {
        let Some(window) = usage.get(source).and_then(Value::as_object) else {
            continue;
        };
        let Some(used_value) = window.get("usedPercent") else {
            continue;
        };
        if !valid_percentage(used_value) {
            continue;
        }
        let used = number_f64(used_value)?;
        let mut converted = serde_json::json!({
            "name": name,
            "usedPercent": used,
            "remainingPercent": python_round(100.0 - used, 6),
        });
        if let Some(reset_at) = window.get("resetsAt").and_then(Value::as_str)
            && reset_at.ends_with('Z')
            && let Ok(parsed) = crate::util::parse_utc_datetime(&Value::from(reset_at))
        {
            converted["resetAtMilliseconds"] = Value::from((parsed * 1000.0) as i64);
        }
        windows.push(converted);
    }
    if windows.is_empty() {
        return None;
    }
    let maximum = windows
        .iter()
        .filter_map(|window| number_f64(&window["usedPercent"]))
        .fold(f64::NEG_INFINITY, f64::max);
    let available = maximum < 100.0;
    Some(serde_json::json!({
        "provider": "claude",
        "available": available,
        "reason": if available { "available-claude-quota" } else { "exhausted" },
        "maxUsedPercent": maximum,
        "quotaWindows": windows,
    }))
}

pub fn provider_quota_status(report: &Value, provider: &Value) -> Result<Value> {
    let usage_provider = provider.get("usageProvider").and_then(Value::as_str);
    let Some(usage_provider) = usage_provider.filter(|text| !text.is_empty()) else {
        return Ok(status(true, None, "unmetered"));
    };
    if let Some(budget) = provider.get("requestBudget")
        && let Some(evaluated) = opencode_go_budget::evaluate(report, usage_provider, Some(budget))?
    {
        return Ok(evaluated);
    }
    if let Some(entry) = find_report_entry(report, Some(usage_provider))
        && let Some(normalized) = claude_quota_entry(Some(entry))
    {
        return Ok(explicitly_reported_status(&normalized));
    }
    Ok(provider_status(report, usage_provider))
}

pub fn native_worker_quota(report: &Value, worker: &Value) -> Result<Value> {
    let usage_provider = worker.get("usageProvider").and_then(Value::as_str);
    let Some(usage_provider) = usage_provider.filter(|text| !text.is_empty()) else {
        return Ok(status(true, None, "unmetered"));
    };
    provider_quota_status(
        report,
        &serde_json::json!({ "usageProvider": usage_provider }),
    )
}

pub fn worker(provider: &Value) -> Value {
    let default_model = provider
        .get("defaultModel")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let model = provider
        .get("subagentModel")
        .and_then(Value::as_str)
        .unwrap_or(default_model);
    let mut result = serde_json::json!({
        "provider": provider.get("id").and_then(Value::as_str).unwrap_or_default(),
        "agent": provider.get("agent").and_then(Value::as_str).unwrap_or_default(),
        "model": model,
        "effort": provider.get("effort").and_then(Value::as_str).unwrap_or_default(),
        "model_prefixes": provider.get("modelPrefixes").cloned().unwrap_or_else(|| Value::Array(vec![])),
    });
    if let Some(maximum) = provider.get("maxConcurrency") {
        result["max_concurrency"] = maximum.clone();
    }
    result
}

pub fn native_worker_item(worker_cfg: &Value) -> Value {
    serde_json::json!({
        "provider": "native",
        "agent": worker_cfg.get("agent").and_then(Value::as_str).unwrap_or_default(),
        "model": worker_cfg.get("model").and_then(Value::as_str).unwrap_or_default(),
        "effort": worker_cfg.get("effort").and_then(Value::as_str).unwrap_or_default(),
    })
}

pub fn selected_native_workers(config: &Config, disabled_models: &BTreeSet<String>) -> Vec<Value> {
    config
        .native_workers
        .iter()
        .filter(|worker_cfg| {
            worker_cfg
                .get("model")
                .and_then(Value::as_str)
                .is_some_and(|model| !disabled_models.contains(model))
        })
        .map(native_worker_item)
        .collect()
}

pub fn capacity_priority(quota: &Value, config_index: i64) -> CapacityKey {
    if quota.get("reason").and_then(Value::as_str) == Some("unmetered") {
        return (1.0, 0.0, 0.0, 0.0, 0.0, 0.0, config_index);
    }
    let (weekly, five_hour) = effective_window_remaining(quota);
    let Some(weekly) = weekly else {
        return (2.0, 0.0, 0.0, 0.0, 0.0, 0.0, config_index);
    };
    (
        0.0,
        -weekly,
        if five_hour.is_some() { 0.0 } else { 1.0 },
        -five_hour.unwrap_or(0.0),
        0.0,
        0.0,
        config_index,
    )
}

pub fn memory_management_enabled() -> bool {
    match env::var(MEMORY_MANAGEMENT_ENV) {
        Ok(raw) => {
            let normalized = raw.trim().to_ascii_lowercase();
            !matches!(normalized.as_str(), "0" | "false" | "off")
        }
        Err(_) => true,
    }
}

fn memory_fraction_env(name: &str, default: f64) -> Result<f64> {
    match env::var(name) {
        Err(_) => Ok(default),
        Ok(raw) => {
            let value: f64 = raw
                .parse()
                .map_err(|_| anyhow::anyhow!("{name} must be a number between 0 and 100"))?;
            if !(0.0..=100.0).contains(&value) {
                anyhow::bail!("{name} must be a number between 0 and 100");
            }
            Ok(value)
        }
    }
}

pub fn memory_pressure_thresholds() -> Result<(f64, f64, f64, f64)> {
    let thresholds = (
        memory_fraction_env(
            MEMORY_AVAILABLE_PCT_CRITICAL_ENV,
            DEFAULT_MEMORY_AVAILABLE_PCT_CRITICAL,
        )?,
        memory_fraction_env(
            MEMORY_AVAILABLE_PCT_LOW_ENV,
            DEFAULT_MEMORY_AVAILABLE_PCT_LOW,
        )?,
        memory_fraction_env(
            MEMORY_AVAILABLE_PCT_MEDIUM_ENV,
            DEFAULT_MEMORY_AVAILABLE_PCT_MEDIUM,
        )?,
        memory_fraction_env(
            MEMORY_AVAILABLE_PCT_MODERATE_ENV,
            DEFAULT_MEMORY_AVAILABLE_PCT_MODERATE,
        )?,
    );
    let sorted = {
        let mut values = [thresholds.0, thresholds.1, thresholds.2, thresholds.3];
        values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        values
    };
    if [thresholds.0, thresholds.1, thresholds.2, thresholds.3] != sorted {
        anyhow::bail!("memory thresholds must be ascending: critical <= low <= medium <= moderate");
    }
    Ok(thresholds)
}

pub fn pressure_level(available_percent: f64, thresholds: (f64, f64, f64, f64)) -> &'static str {
    let (critical, low, medium, moderate) = thresholds;
    if available_percent < critical {
        "critical"
    } else if available_percent < low {
        "high"
    } else if available_percent < medium {
        "medium"
    } else if available_percent < moderate {
        "moderate"
    } else {
        "ok"
    }
}

pub fn memory_parallel_cap(
    available_percent: f64,
    thresholds: (f64, f64, f64, f64),
) -> Option<i64> {
    match pressure_level(available_percent, thresholds) {
        "critical" => Some(1),
        "high" => Some(2),
        "medium" => Some(4),
        "moderate" => Some(8),
        _ => None,
    }
}

pub fn orchestration_settings() -> Result<Map<String, Value>> {
    let max_parallel = env::var(SUBAGENT_MAX_PARALLEL_ENV)
        .ok()
        .or_else(|| env::var("CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS").ok());
    let mut settings = Map::new();
    settings.insert(
        "max_parallel_workers".into(),
        Value::from(positive_or_default(
            max_parallel.as_deref(),
            SUBAGENT_MAX_PARALLEL_ENV,
            DEFAULT_MAX_SUBAGENTS,
            1,
        )?),
    );
    settings.insert("minimum_subagents_per_phase".into(), Value::from(1));
    settings.insert("minimum_active_subagents".into(), Value::from(1));
    settings.insert(
        "reevaluate_on_completion".into(),
        Value::from(boolean_env(SUBAGENT_REEVALUATE_ON_COMPLETION_ENV, true)?),
    );
    settings.insert(
        "monitor_interval_seconds".into(),
        Value::from(positive_or_default(
            env::var(SUBAGENT_REASSESS_INTERVAL_ENV).ok().as_deref(),
            SUBAGENT_REASSESS_INTERVAL_ENV,
            ORCHESTRATION_REBALANCE_INTERVAL_SECONDS,
            1,
        )?),
    );
    settings.insert("minimum_model_kinds".into(), Value::from(1));
    settings.insert(
        "reuse_compatible_workers".into(),
        Value::from(boolean_env(SUBAGENT_REUSE_ENV, true)?),
    );
    settings.insert(
        "cleanup_on_exit".into(),
        Value::from(boolean_env(SUBAGENT_CLEANUP_ON_EXIT_ENV, true)?),
    );
    settings.insert(
        "subagent_first".into(),
        Value::from(boolean_env(SUBAGENT_FIRST_ENV, true)?),
    );
    settings.insert(
        "status_poll_interval_seconds".into(),
        Value::from(positive_or_default(
            env::var(SUBAGENT_STATUS_POLL_ENV).ok().as_deref(),
            SUBAGENT_STATUS_POLL_ENV,
            DEFAULT_SUBAGENT_STATUS_POLL_SECONDS,
            1,
        )?),
    );
    Ok(settings)
}

pub fn effective_orchestration_settings(summary: &Value) -> Result<Map<String, Value>> {
    let settings = orchestration_settings()?;
    let memory = summary.get("memory_status");
    let Some(memory) = memory.filter(|value| {
        value.get("status").and_then(Value::as_str) == Some("available")
            && value
                .get("available_percent")
                .and_then(number_f64)
                .is_some()
    }) else {
        return Ok(settings);
    };
    let mut effective = settings.clone();
    let thresholds = memory_pressure_thresholds()?;
    let available_percent = number_f64(&memory["available_percent"]).unwrap_or_default();
    if let Some(cap) = memory_parallel_cap(available_percent, thresholds) {
        let configured = settings["max_parallel_workers"]
            .as_i64()
            .unwrap_or(DEFAULT_MAX_SUBAGENTS);
        effective.insert(
            "max_parallel_workers".into(),
            Value::from(configured.min(cap)),
        );
    }
    if matches!(
        memory.get("pressure_level").and_then(Value::as_str),
        Some("critical" | "high")
    ) {
        effective.insert("reuse_compatible_workers".into(), Value::Bool(true));
    }
    Ok(effective)
}

pub fn memory_management_contract(summary: &Value, settings: &Map<String, Value>) -> Result<Value> {
    let memory = summary
        .get("memory_status")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    let status_name = memory
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if matches!(status_name, "available" | "disabled" | "unavailable") {
        let mut contract = serde_json::json!({
            "status": status_name,
            "pressure_level": memory.get("pressure_level").cloned().unwrap_or(Value::Null),
            "available_percent": memory.get("available_percent").cloned().unwrap_or(Value::Null),
            "total_mb": memory.get("total_mb").cloned().unwrap_or(Value::Null),
            "available_mb": memory.get("available_mb").cloned().unwrap_or(Value::Null),
            "configured_max_parallel_workers": Value::Null,
            "effective_max_parallel_workers": settings.get("max_parallel_workers").cloned().unwrap_or(Value::Null),
            "reuse_required": false,
            "management_active": false,
        });
        if status_name == "available" {
            let configured = orchestration_settings()?["max_parallel_workers"].clone();
            contract["configured_max_parallel_workers"] = configured.clone();
            let effective = settings
                .get("max_parallel_workers")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let configured_i = configured.as_i64().unwrap_or_default();
            contract["management_active"] = Value::Bool(effective < configured_i);
            contract["reuse_required"] = Value::Bool(matches!(
                memory.get("pressure_level").and_then(Value::as_str),
                Some("critical" | "high")
            ));
        }
        return Ok(contract);
    }
    Ok(serde_json::json!({
        "status": "unknown",
        "pressure_level": Value::Null,
        "available_percent": Value::Null,
        "total_mb": Value::Null,
        "available_mb": Value::Null,
        "configured_max_parallel_workers": Value::Null,
        "effective_max_parallel_workers": settings.get("max_parallel_workers").cloned().unwrap_or(Value::Null),
        "reuse_required": false,
        "management_active": false,
    }))
}

pub fn task_fanout(
    independent_scopes: i64,
    available_workers: i64,
    summary: Option<&Value>,
) -> Result<i64> {
    if independent_scopes < 0 || available_workers < 0 {
        anyhow::bail!("scope and worker counts must be non-negative integers");
    }
    let settings = match summary {
        Some(summary) => effective_orchestration_settings(summary)?,
        None => orchestration_settings()?,
    };
    let max_parallel = settings["max_parallel_workers"]
        .as_i64()
        .unwrap_or(DEFAULT_MAX_SUBAGENTS);
    Ok(independent_scopes.min(available_workers).min(max_parallel))
}

pub fn orchestration_contract(summary: &Value) -> Result<Value> {
    let workers = summary
        .get("selected_workers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let models: BTreeSet<String> = workers
        .iter()
        .filter_map(|worker| worker.get("model").and_then(Value::as_str))
        .filter(|model| !model.is_empty())
        .map(model_family)
        .collect();
    let available = workers.len() as i64;
    let settings = effective_orchestration_settings(summary)?;
    let mut contract = Value::Object(settings.clone());
    let object = contract.as_object_mut().expect("object");
    object.insert("dynamic_fanout".into(), Value::Bool(true));
    object.insert("max_available_workers".into(), Value::from(available));
    object.insert(
        "fanout_rule".into(),
        Value::from("min(independent_scopes, max_available_workers, max_parallel_workers)"),
    );
    object.insert(
        "task_fanout_default".into(),
        Value::from(task_fanout(1, available, Some(summary))?),
    );
    object.insert(
        "available_model_kinds".into(),
        Value::from(models.len() as i64),
    );
    let minimum_kinds = settings["minimum_model_kinds"].as_i64().unwrap_or(1);
    object.insert(
        "model_diversity_satisfied".into(),
        Value::Bool(models.len() as i64 >= minimum_kinds),
    );
    object.insert(
        "completion_rebalance_required".into(),
        settings["reevaluate_on_completion"].clone(),
    );
    object.insert("custom_advisor_exempt".into(), Value::Bool(true));
    object.insert(
        "custom_advisor_consult_when".into(),
        Value::Array(
            CUSTOM_ADVISOR_CONSULT_WHEN
                .iter()
                .map(|item| Value::from(*item))
                .collect(),
        ),
    );
    let minimum_phase = settings["minimum_subagents_per_phase"]
        .as_i64()
        .unwrap_or(1);
    object.insert(
        "capacity_shortfall".into(),
        Value::Bool(available < minimum_phase),
    );
    object.insert("hook_launches_agents".into(), Value::Bool(false));
    object.insert("background_status_required".into(), Value::Bool(true));
    object.insert(
        "memory_management".into(),
        memory_management_contract(summary, &settings)?,
    );
    let excluded = summary
        .get("automatic_selection_excluded_models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut sorted_excluded = excluded
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    sorted_excluded.sort();
    object.insert(
        "automatic_selection_excluded_models".into(),
        Value::Array(sorted_excluded.into_iter().map(Value::from).collect()),
    );
    object.insert(
        "sonnet_subagent_suppressed".into(),
        Value::Bool(
            summary
                .get("sonnet_subagent_suppressed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
    );
    Ok(contract)
}

pub fn routing_summary(
    report: &Value,
    config: &Config,
    disabled_models: &BTreeSet<String>,
) -> Result<Value> {
    let mut providers = Map::new();
    let mut candidates: Vec<(CapacityKey, Value)> = Vec::new();
    for (index, provider) in config.providers.iter().enumerate() {
        let usage_provider = provider.get("usageProvider").and_then(Value::as_str);
        let entry = find_report_entry(report, usage_provider);
        let normalized = claude_quota_entry(entry);
        let mut quota = provider_quota_status(report, provider)?;
        quota["quota_windows"] = quota_window_remaining(normalized.as_ref().or(entry));
        let worker_item = worker(provider);
        let model = worker_item
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let disabled = disabled_models.contains(model);
        let effective = if disabled {
            status(false, None, "disabled-by-policy")
        } else {
            quota.clone()
        };
        let provider_id = provider
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut fields = effective.as_object().cloned().unwrap_or_default();
        if let Some(object) = worker_item.as_object() {
            fields.extend(object.clone());
        }
        fields.insert("disabled".into(), Value::Bool(disabled));
        providers.insert(provider_id.to_owned(), Value::Object(fields));
        if quota
            .get("available")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && !disabled
        {
            candidates.push((capacity_priority(&quota, index as i64), worker_item));
        }
    }
    let mut native_quota = Map::new();
    for (native_index, native) in config.native_workers.iter().enumerate() {
        let native_item = native_worker_item(native);
        let model = native_item
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if disabled_models.contains(model) {
            continue;
        }
        let usage_provider = native.get("usageProvider").and_then(Value::as_str);
        let entry = find_report_entry(report, usage_provider);
        let normalized = claude_quota_entry(entry);
        let mut quota = native_worker_quota(report, native)?;
        quota["quota_windows"] = quota_window_remaining(normalized.as_ref().or(entry));
        let agent = native
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or_default();
        native_quota.insert(agent.to_owned(), quota.clone());
        if quota
            .get("available")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && quota.get("max_used_percent").and_then(number_f64).is_some()
        {
            candidates.push((
                capacity_priority(&quota, (config.providers.len() + native_index) as i64),
                native_item,
            ));
        }
    }
    candidates.sort_by(|left, right| {
        left.0
            .partial_cmp(&right.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut selected: Vec<Value> = candidates.into_iter().map(|(_, item)| item).collect();
    let fallback_active = selected.is_empty()
        && config
            .fallback
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|model| !disabled_models.contains(model));
    if fallback_active {
        let mut fallback = config.fallback.clone();
        if let Some(object) = fallback.as_object_mut() {
            object.insert("provider".into(), Value::from("fallback"));
        }
        selected = vec![fallback];
    }
    let participating: BTreeSet<String> = selected
        .iter()
        .filter_map(|item| item.get("agent").and_then(Value::as_str).map(str::to_owned))
        .collect();
    selected.extend(
        selected_native_workers(config, disabled_models)
            .into_iter()
            .filter(|item| {
                item.get("agent")
                    .and_then(Value::as_str)
                    .is_none_or(|agent| !participating.contains(agent))
            }),
    );
    let preferred = selected.first().cloned();
    let mut summary = serde_json::json!({
        "providers": providers,
        "native_worker_quota": native_quota,
        "selected_agents": selected.iter().filter_map(|item| item.get("agent").cloned()).collect::<Vec<_>>(),
        "selected_workers": selected,
        "preferred_worker": preferred,
        "fallback_active": fallback_active,
        "disabled_subagent_models": disabled_models.iter().cloned().collect::<Vec<_>>(),
        "advisor": config.advisor.clone(),
    });
    summary["orchestration"] = orchestration_contract(&summary)?;
    Ok(summary)
}

pub fn combined_capacity_priority(
    quota: &Value,
    concurrency: &Value,
    config_index: i64,
) -> CapacityKey {
    let unmetered = quota.get("reason").and_then(Value::as_str) == Some("unmetered");
    let (mut weekly, mut five_hour) = effective_window_remaining(quota);
    let tier = if unmetered {
        1.0
    } else if weekly.is_none() {
        2.0
    } else {
        0.0
    };
    let mut parallel_used = 0.0;
    if let (Some(active), Some(queued), Some(limit)) = (
        concurrency.get("active").and_then(Value::as_i64),
        concurrency.get("queued").and_then(Value::as_i64),
        concurrency.get("limit").and_then(Value::as_i64),
    ) && limit != 0
    {
        parallel_used = 100.0 * (active + queued) as f64 / limit as f64;
    }
    if parallel_used != 0.0 {
        let parallel_remaining = 100.0 - parallel_used;
        weekly = Some(weekly.map_or(parallel_remaining, |value| value.min(parallel_remaining)));
        five_hour =
            Some(five_hour.map_or(parallel_remaining, |value| value.min(parallel_remaining)));
    }
    let health_unknown =
        if concurrency.get("reason").and_then(Value::as_str) == Some("daemon-health-unavailable") {
            1.0
        } else {
            0.0
        };
    (
        tier,
        if weekly.is_some() { 0.0 } else { 1.0 },
        -weekly.unwrap_or(0.0),
        if five_hour.is_some() { 0.0 } else { 1.0 },
        -five_hour.unwrap_or(0.0),
        health_unknown,
        config_index,
    )
}

pub fn concurrency_status(
    active: Option<i64>,
    queued: Option<i64>,
    limit: Option<i64>,
    available: bool,
    reason: &str,
    known: bool,
) -> Value {
    serde_json::json!({
        "active": active,
        "queued": queued,
        "limit": limit,
        "available": available,
        "remaining": match (active, queued, limit) {
            (Some(active), Some(queued), Some(limit)) => Value::from((limit - active - queued).max(0)),
            _ => Value::Null,
        },
        "reason": reason,
        "known": known,
    })
}

pub fn provider_for_model<'a>(config: &'a Config, model: &str) -> Option<&'a Value> {
    let exact: Vec<&Value> = config
        .providers
        .iter()
        .filter(|provider| {
            let default = provider.get("defaultModel").and_then(Value::as_str);
            let subagent = provider
                .get("subagentModel")
                .and_then(Value::as_str)
                .or(default);
            default == Some(model) || subagent == Some(model)
        })
        .collect();
    if let Some(first) = exact.first() {
        return Some(*first);
    }
    let mut matches: Vec<(usize, i64, &Value)> = Vec::new();
    for (index, provider) in config.providers.iter().enumerate() {
        let prefixes = provider
            .get("modelPrefixes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for prefix in prefixes {
            if let Some(prefix) = prefix.as_str()
                && model.starts_with(prefix)
            {
                matches.push((prefix.len(), -(index as i64), provider));
            }
        }
    }
    matches.sort_by(|left, right| right.0.cmp(&left.0).then(right.1.cmp(&left.1)));
    matches.first().map(|item| item.2)
}

pub fn model_concurrency_status(
    provider: &Value,
    model: &str,
    health: Option<&BTreeMap<String, Value>>,
) -> Value {
    let configured_limit = provider.get("maxConcurrency").and_then(Value::as_i64);
    let Some(configured_limit) = configured_limit else {
        return concurrency_status(None, None, None, true, "not-limited", true);
    };
    let Some(health) = health else {
        return concurrency_status(
            None,
            None,
            Some(configured_limit),
            true,
            "daemon-health-unavailable",
            false,
        );
    };
    let Some(fields) = health.get(model) else {
        return concurrency_status(Some(0), Some(0), Some(configured_limit), true, "idle", true);
    };
    let active = fields
        .get("active")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let queued = fields
        .get("queued")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let limit = fields
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    if limit != configured_limit {
        let available = active + queued < configured_limit;
        return concurrency_status(
            Some(active),
            Some(queued),
            Some(configured_limit),
            available,
            "configured-limit-mismatch",
            false,
        );
    }
    let available = fields
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    concurrency_status(
        Some(active),
        Some(queued),
        Some(configured_limit),
        available,
        if available {
            "available"
        } else {
            "limit-reached"
        },
        true,
    )
}

pub fn apply_model_concurrency(
    summary: Value,
    config: &Config,
    health: Option<&BTreeMap<String, Value>>,
    disabled_models: &BTreeSet<String>,
) -> Result<Value> {
    let mut combined = summary;
    let mut candidates: Vec<(CapacityKey, Value)> = Vec::new();
    let mut model_capacity = Map::new();
    for (index, provider) in config.providers.iter().enumerate() {
        let provider_id = provider
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(fields) = combined
            .get_mut("providers")
            .and_then(Value::as_object_mut)
            .and_then(|providers| providers.get_mut(provider_id))
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        let current_worker = worker(provider);
        let model = current_worker
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let concurrency = model_concurrency_status(provider, &model, health);
        if provider.get("maxConcurrency").is_some() {
            model_capacity.insert(model.clone(), concurrency.clone());
            fields.insert("concurrency_active".into(), concurrency["active"].clone());
            fields.insert("concurrency_queued".into(), concurrency["queued"].clone());
            fields.insert("concurrency_limit".into(), concurrency["limit"].clone());
            fields.insert(
                "concurrency_remaining".into(),
                concurrency["remaining"].clone(),
            );
            fields.insert(
                "concurrency_available".into(),
                concurrency["available"].clone(),
            );
            fields.insert("concurrency_reason".into(), concurrency["reason"].clone());
        }
        let quota = serde_json::json!({
            "available": fields.get("available").cloned().unwrap_or(Value::Bool(false)),
            "max_used_percent": fields.get("max_used_percent").cloned().unwrap_or(Value::Null),
            "reason": fields.get("reason").cloned().unwrap_or(Value::Null),
            "quota_windows": fields.get("quota_windows").cloned().unwrap_or_else(|| Value::Object(Map::new())),
        });
        let disabled = disabled_models.contains(&model)
            || fields
                .get("disabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let quota_available = quota
            .get("available")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let concurrency_available = concurrency
            .get("available")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if quota_available && !concurrency_available && !disabled {
            fields.insert("available".into(), Value::Bool(false));
            fields.insert("reason".into(), Value::from("concurrency-limit-reached"));
        }
        if quota_available && concurrency_available && !disabled {
            let mut selected_worker = current_worker;
            if provider.get("maxConcurrency").is_some()
                && let Some(object) = selected_worker.as_object_mut()
            {
                object.insert("concurrency".into(), concurrency.clone());
            }
            candidates.push((
                combined_capacity_priority(&quota, &concurrency, index as i64),
                selected_worker,
            ));
        }
    }
    if let Some(health) = health {
        for model in health.keys() {
            if let Some(provider) = provider_for_model(config, model)
                && provider.get("maxConcurrency").is_some()
            {
                let worker_item = worker(provider);
                let worker_model = worker_item
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let default_model = provider
                    .get("defaultModel")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !(model == default_model && model != worker_model) {
                    model_capacity.insert(
                        model.clone(),
                        model_concurrency_status(provider, model, Some(health)),
                    );
                }
            }
        }
    }
    let native_quota = combined
        .get("native_worker_quota")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut participating_natives = BTreeSet::new();
    for (native_index, native) in config.native_workers.iter().enumerate() {
        let native_item = native_worker_item(native);
        let model = native_item
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if disabled_models.contains(model) {
            continue;
        }
        let agent = native
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(quota) = native_quota.get(agent) else {
            continue;
        };
        if quota
            .get("available")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && quota.get("max_used_percent").and_then(number_f64).is_some()
        {
            participating_natives.insert(agent.to_owned());
            candidates.push((
                combined_capacity_priority(
                    quota,
                    &concurrency_status(None, None, None, true, "not-limited", true),
                    (config.providers.len() + native_index) as i64,
                ),
                native_item,
            ));
        }
    }
    candidates.sort_by(|left, right| {
        left.0
            .partial_cmp(&right.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut selected: Vec<Value> = candidates.into_iter().map(|(_, item)| item).collect();
    let mut fallback = config.fallback.clone();
    if let Some(object) = fallback.as_object_mut() {
        object.insert("provider".into(), Value::from("fallback"));
    }
    let fallback_model = fallback
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let fallback_active = selected.is_empty() && !disabled_models.contains(fallback_model);
    if fallback_active {
        selected = vec![fallback];
    }
    selected.extend(
        selected_native_workers(config, disabled_models)
            .into_iter()
            .filter(|item| {
                item.get("agent")
                    .and_then(Value::as_str)
                    .is_none_or(|agent| !participating_natives.contains(agent))
            }),
    );
    let preferred = selected.first().cloned();
    if let Some(object) = combined.as_object_mut() {
        object.insert(
            "selected_agents".into(),
            Value::Array(
                selected
                    .iter()
                    .filter_map(|item| item.get("agent").cloned())
                    .collect(),
            ),
        );
        object.insert("selected_workers".into(), Value::Array(selected));
        object.insert("preferred_worker".into(), preferred.unwrap_or(Value::Null));
        object.insert("fallback_active".into(), Value::Bool(fallback_active));
        object.insert("model_concurrency".into(), Value::Object(model_capacity));
    }
    let orchestration = orchestration_contract(&combined)?;
    if let Some(object) = combined.as_object_mut() {
        object.insert("orchestration".into(), orchestration);
    }
    Ok(combined)
}

pub fn worker_capacity_metadata(summary: &Value) -> Vec<Value> {
    let provider_status = summary
        .get("providers")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let native_quota = summary
        .get("native_worker_quota")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut capacity = Vec::new();
    let workers = summary
        .get("selected_workers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for worker_item in workers {
        let Some(object) = worker_item.as_object() else {
            continue;
        };
        let mut entry = Map::new();
        for key in ["agent", "model"] {
            if let Some(value) = object.get(key) {
                entry.insert(key.to_owned(), value.clone());
            }
        }
        let mut quota = object
            .get("provider")
            .and_then(Value::as_str)
            .and_then(|provider| provider_status.get(provider));
        if quota.is_none() && object.get("provider").and_then(Value::as_str) == Some("native") {
            quota = object
                .get("agent")
                .and_then(Value::as_str)
                .and_then(|agent| native_quota.get(agent));
        }
        if let Some(quota) =
            quota.filter(|value| value.get("max_used_percent").and_then(number_f64).is_some())
        {
            let used = number_f64(&quota["max_used_percent"]).unwrap_or_default();
            entry.insert("used_percent".into(), Value::from(used));
            entry.insert(
                "remaining_percent".into(),
                Value::from(python_round(100.0 - used, 1)),
            );
        } else {
            entry.insert("used_percent".into(), Value::Null);
            entry.insert("remaining_percent".into(), Value::Null);
        }
        let (weekly, five_hour) =
            effective_window_remaining(quota.unwrap_or(&Value::Object(Map::new())));
        entry.insert(
            "weekly_remaining_percent".into(),
            weekly.map_or(Value::Null, |value| Value::from(python_round(value, 1))),
        );
        entry.insert(
            "five_hour_remaining_percent".into(),
            five_hour.map_or(Value::Null, |value| Value::from(python_round(value, 1))),
        );
        capacity.push(Value::Object(entry));
    }
    capacity
}

pub fn ranked_worker_metadata(summary: &Value) -> Vec<Value> {
    worker_capacity_metadata(summary)
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            serde_json::json!({
                "rank": index + 1,
                "agent": entry.get("agent").cloned().unwrap_or(Value::Null),
                "model": entry.get("model").cloned().unwrap_or(Value::Null),
                "weekly_remaining_percent": entry.get("weekly_remaining_percent").cloned().unwrap_or(Value::Null),
                "five_hour_remaining_percent": entry.get("five_hour_remaining_percent").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

pub fn default_subagent_route(summary: &Value) -> Option<Value> {
    let workers = summary.get("selected_workers")?.as_array()?;
    let top = workers.first()?.as_object()?;
    if top
        .get("agent")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
        || top
            .get("model")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return None;
    }
    let mut route = Map::new();
    for key in ["agent", "model", "effort"] {
        if let Some(value) = top.get(key) {
            route.insert(key.to_owned(), value.clone());
        }
    }
    route.insert(
        "applies_to_subagent_types".into(),
        Value::Array(vec![Value::from("general-purpose")]),
    );
    route.insert(
        "applies_when_claudex_model_omitted".into(),
        Value::Bool(true),
    );
    Some(Value::Object(route))
}

pub fn enforce_worker_model_separation(
    summary: Value,
    main_model: Option<&str>,
    main_model_known: bool,
    allow_sonnet_subagent: bool,
) -> Result<Value> {
    let mut separated = summary;
    let selected = separated
        .get("selected_workers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let current_main_model = if main_model_known { main_model } else { None };
    let current_main_model_known = main_model_known && current_main_model.is_some();
    let mut excluded_models = BTreeSet::new();
    let mut sonnet_suppressed = false;
    let selected = if is_sonnet_model(current_main_model) && !allow_sonnet_subagent {
        let mut retained = Vec::new();
        for worker_item in selected {
            if is_sonnet_model(worker_item.get("model").and_then(Value::as_str)) {
                if let Some(model) = worker_item.get("model").and_then(Value::as_str) {
                    excluded_models.insert(model.to_owned());
                }
                sonnet_suppressed = true;
            } else {
                retained.push(worker_item);
            }
        }
        retained
    } else {
        selected
    };
    let preferred = selected.first().cloned();
    if let Some(object) = separated.as_object_mut() {
        object.insert(
            "selected_agents".into(),
            Value::Array(
                selected
                    .iter()
                    .filter_map(|item| item.get("agent").cloned())
                    .collect(),
            ),
        );
        object.insert("selected_workers".into(), Value::Array(selected.clone()));
        object.insert("preferred_worker".into(), preferred.unwrap_or(Value::Null));
        object.insert(
            "current_main_model".into(),
            current_main_model.map_or(Value::Null, Value::from),
        );
        object.insert(
            "current_main_model_known".into(),
            Value::Bool(current_main_model_known),
        );
        object.insert(
            "main_session_model".into(),
            current_main_model.map_or(Value::Null, Value::from),
        );
        object.insert(
            "automatic_selection_excluded_models".into(),
            Value::Array(excluded_models.into_iter().map(Value::from).collect()),
        );
        object.insert(
            "sonnet_subagent_suppressed".into(),
            Value::Bool(sonnet_suppressed),
        );
        object.insert(
            "sonnet_subagent_explicit_allowed".into(),
            Value::Bool(allow_sonnet_subagent),
        );
        if sonnet_suppressed {
            object.insert("fallback_active".into(), Value::Bool(false));
        }
        object.insert("orchestration_mode".into(), Value::from("subagent-first"));
    }
    let orchestration = orchestration_contract(&separated)?;
    let delegation_required = !selected.is_empty()
        && orchestration
            .get("subagent_first")
            .and_then(Value::as_bool)
            .unwrap_or(true);
    if let Some(object) = separated.as_object_mut() {
        object.insert("orchestration".into(), orchestration);
        object.insert(
            "delegation_required".into(),
            Value::Bool(delegation_required),
        );
        object.insert(
            "direct_main_execution".into(),
            Value::from(if delegation_required {
                "fallback-only"
            } else {
                "allowed"
            }),
        );
        object.insert("background_status_required".into(), Value::Bool(true));
    }
    let orchestration = orchestration_contract(&separated)?;
    if let Some(object) = separated.as_object_mut() {
        object.insert("orchestration".into(), orchestration);
    }
    Ok(separated)
}

pub fn fallback_summary(
    reason: &str,
    config: &Config,
    disabled_models: &BTreeSet<String>,
) -> Result<Value> {
    let mut providers = Map::new();
    for provider in &config.providers {
        let worker_item = worker(provider);
        let model = worker_item
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let disabled = disabled_models.contains(model);
        let unavailable_reason = if disabled {
            "disabled-by-policy"
        } else {
            reason
        };
        let mut fields = status(false, None, unavailable_reason)
            .as_object()
            .cloned()
            .unwrap_or_default();
        if let Some(object) = worker_item.as_object() {
            fields.extend(object.clone());
        }
        fields.insert("disabled".into(), Value::Bool(disabled));
        let provider_id = provider
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        providers.insert(provider_id.to_owned(), Value::Object(fields));
    }
    let mut fallback = config.fallback.clone();
    if let Some(object) = fallback.as_object_mut() {
        object.insert("provider".into(), Value::from("fallback"));
    }
    let fallback_model = fallback
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut selected = if disabled_models.contains(fallback_model) {
        Vec::new()
    } else {
        vec![fallback]
    };
    let fallback_active = !selected.is_empty();
    selected.extend(selected_native_workers(config, disabled_models));
    let preferred = selected.first().cloned();
    let mut summary = serde_json::json!({
        "providers": providers,
        "selected_agents": selected.iter().filter_map(|item| item.get("agent").cloned()).collect::<Vec<_>>(),
        "selected_workers": selected,
        "preferred_worker": preferred,
        "fallback_active": fallback_active,
        "disabled_subagent_models": disabled_models.iter().cloned().collect::<Vec<_>>(),
        "advisor": config.advisor.clone(),
    });
    summary["orchestration"] = orchestration_contract(&summary)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load_config;
    use serde_json::json;
    use std::collections::BTreeSet;
    use tempfile::NamedTempFile;

    fn sample_config() -> Config {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
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
                  "usageProvider": "qwen",
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
        .unwrap();
        load_config(file.path()).unwrap()
    }

    fn report() -> Value {
        json!([
            {"provider":"codex","usage":{"primary":{"usedPercent":10},"secondary":{"usedPercent":20}}},
            {"provider":"grok","usage":{"primary":{"usedPercent":40}}},
            {
              "provider":"qwen",
              "available": true,
              "reason": "available-qwen-cloud-quota",
              "maxUsedPercent": 30,
              "quotaWindows": [
                {"name":"five-hour","usedPercent":20,"remainingPercent":80},
                {"name":"seven-day","usedPercent":30,"remainingPercent":70}
              ]
            }
        ])
    }

    #[test]
    fn selects_available_workers_and_exposes_orchestration() {
        let summary = routing_summary(&report(), &sample_config(), &BTreeSet::new()).unwrap();
        let agents = summary["selected_agents"].as_array().unwrap();
        assert!(agents.iter().any(|a| a == "claudex-qwen"));
        assert!(agents.iter().any(|a| a == "claudex-gpt-spark"));
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
            {"provider":"qwen","available":false,"reason":"exhausted","maxUsedPercent":100}
        ]);
        let summary = routing_summary(&usage, &sample_config(), &BTreeSet::new()).unwrap();
        assert_eq!(summary["selected_workers"][0]["agent"], "claudex-grok");
        assert_eq!(summary["selected_workers"][1]["agent"], "claudex-gpt-spark");
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
        let models: Vec<_> = summary["selected_workers"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|w| w.get("model").and_then(Value::as_str))
            .collect();
        assert!(!models.contains(&"gpt-5.3-codex-spark"));
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
        assert_eq!(memory_parallel_cap(5.0, thresholds), Some(1));
        assert_eq!(memory_parallel_cap(15.0, thresholds), Some(2));
        assert_eq!(memory_parallel_cap(25.0, thresholds), Some(4));
        assert_eq!(memory_parallel_cap(35.0, thresholds), Some(8));
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
        let normalized = claude_quota_entry(Some(&entry)).unwrap();
        assert_eq!(normalized["reason"], "available-claude-quota");
        assert_eq!(normalized["quotaWindows"][0]["name"], "five-hour");
        assert_eq!(normalized["quotaWindows"][1]["name"], "seven-day");
    }
}
