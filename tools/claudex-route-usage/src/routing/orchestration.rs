//! Orchestration settings, memory-pressure contract, and fan-out policy.

use super::{
    CUSTOM_ADVISOR_CONSULT_WHEN, DEFAULT_ACTIVE_SUBAGENT_FLOOR, DEFAULT_MAX_SUBAGENTS,
    DEFAULT_MIN_MODEL_KINDS, DEFAULT_MIN_SUBAGENTS_PER_PHASE, DEFAULT_SUBAGENT_STATUS_POLL_SECONDS,
    ORCHESTRATION_REBALANCE_INTERVAL_SECONDS, SUBAGENT_ACTIVE_FLOOR_ENV,
    SUBAGENT_CLEANUP_ON_EXIT_ENV, SUBAGENT_FIRST_ENV, SUBAGENT_MAX_PARALLEL_ENV,
    SUBAGENT_MIN_MODEL_FAMILIES_ENV, SUBAGENT_MIN_PARALLEL_ENV, SUBAGENT_REASSESS_INTERVAL_ENV,
    SUBAGENT_REEVALUATE_ON_COMPLETION_ENV, SUBAGENT_REUSE_ENV, SUBAGENT_STATUS_POLL_ENV,
    memory_parallel_cap, memory_pressure_thresholds,
};
use crate::util::{boolean_env, model_family, number_f64, positive_or_default};
use anyhow::Result;
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::env;

pub fn orchestration_settings() -> Result<Map<String, Value>> {
    let max_parallel = env::var(SUBAGENT_MAX_PARALLEL_ENV)
        .ok()
        .or_else(|| env::var("CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS").ok());
    let max_parallel_workers = positive_or_default(
        max_parallel.as_deref(),
        SUBAGENT_MAX_PARALLEL_ENV,
        DEFAULT_MAX_SUBAGENTS,
        1,
    )?;
    let min_phase = positive_or_default(
        env::var(SUBAGENT_MIN_PARALLEL_ENV).ok().as_deref(),
        SUBAGENT_MIN_PARALLEL_ENV,
        DEFAULT_MIN_SUBAGENTS_PER_PHASE,
        1,
    )?
    .min(max_parallel_workers);
    let active_floor = positive_or_default(
        env::var(SUBAGENT_ACTIVE_FLOOR_ENV).ok().as_deref(),
        SUBAGENT_ACTIVE_FLOOR_ENV,
        DEFAULT_ACTIVE_SUBAGENT_FLOOR,
        1,
    )?
    .min(max_parallel_workers)
    .min(min_phase);
    let min_model_kinds = positive_or_default(
        env::var(SUBAGENT_MIN_MODEL_FAMILIES_ENV).ok().as_deref(),
        SUBAGENT_MIN_MODEL_FAMILIES_ENV,
        DEFAULT_MIN_MODEL_KINDS,
        1,
    )?;
    let mut settings = Map::new();
    settings.insert(
        "max_parallel_workers".into(),
        Value::from(max_parallel_workers),
    );
    settings.insert("minimum_subagents_per_phase".into(), Value::from(min_phase));
    settings.insert("minimum_active_subagents".into(), Value::from(active_floor));
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
    settings.insert("minimum_model_kinds".into(), Value::from(min_model_kinds));
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
    let Some(memory) = memory.filter(|value| reports_available_memory(value)) else {
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
    if under_memory_pressure(memory) {
        effective.insert("reuse_compatible_workers".into(), Value::Bool(true));
    }
    Ok(effective)
}

fn reports_available_memory(memory: &Value) -> bool {
    memory.get("status").and_then(Value::as_str) == Some("available")
        && memory
            .get("available_percent")
            .and_then(number_f64)
            .is_some()
}

fn under_memory_pressure(memory: &Value) -> bool {
    matches!(
        memory.get("pressure_level").and_then(Value::as_str),
        Some("critical" | "high")
    )
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
    if !matches!(status_name, "available" | "disabled" | "unavailable") {
        return Ok(memory_contract_shape("unknown", &Value::Null, settings));
    }
    let mut contract = memory_contract_shape(status_name, &memory, settings);
    if status_name == "available" {
        annotate_active_management(&mut contract, &memory, settings)?;
    }
    Ok(contract)
}

fn memory_contract_shape(
    status_name: &str,
    memory: &Value,
    settings: &Map<String, Value>,
) -> Value {
    serde_json::json!({
        "status": status_name,
        "pressure_level": memory.get("pressure_level").cloned().unwrap_or(Value::Null),
        "available_percent": memory.get("available_percent").cloned().unwrap_or(Value::Null),
        "total_mb": memory.get("total_mb").cloned().unwrap_or(Value::Null),
        "available_mb": memory.get("available_mb").cloned().unwrap_or(Value::Null),
        "configured_max_parallel_workers": Value::Null,
        "effective_max_parallel_workers": settings.get("max_parallel_workers").cloned().unwrap_or(Value::Null),
        "reuse_required": false,
        "management_active": false,
    })
}

fn annotate_active_management(
    contract: &mut Value,
    memory: &Value,
    settings: &Map<String, Value>,
) -> Result<()> {
    let configured = orchestration_settings()?["max_parallel_workers"].clone();
    contract["configured_max_parallel_workers"] = configured.clone();
    let effective = settings
        .get("max_parallel_workers")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    contract["management_active"] =
        Value::Bool(effective < configured.as_i64().unwrap_or_default());
    contract["reuse_required"] = Value::Bool(under_memory_pressure(memory));
    Ok(())
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
    let model_kinds: BTreeSet<String> = workers
        .iter()
        .filter_map(|worker| worker.get("model").and_then(Value::as_str))
        .filter(|model| !model.is_empty())
        .map(model_family)
        .collect();
    let available = workers.len() as i64;
    let settings = effective_orchestration_settings(summary)?;
    let mut contract = Value::Object(settings.clone());
    let object = contract.as_object_mut().expect("object");
    insert_fanout_contract(object, summary, available)?;
    insert_diversity_contract(object, &settings, model_kinds.len() as i64);
    insert_policy_contract(object, summary, &settings, available)?;
    Ok(contract)
}

fn insert_fanout_contract(
    object: &mut Map<String, Value>,
    summary: &Value,
    available: i64,
) -> Result<()> {
    object.insert("dynamic_fanout".into(), Value::Bool(true));
    object.insert(
        "fanout_matches_independent_scopes".into(),
        Value::Bool(true),
    );
    object.insert("max_available_workers".into(), Value::from(available));
    object.insert(
        "fanout_rule".into(),
        Value::from("min(independent_scopes, max_available_workers, max_parallel_workers)"),
    );
    // Single-scope baseline only. Never treat this as a hard launch size for
    // multi-scope work — use task_fanout_examples / task_fanout(scopes, …).
    let single_scope_fanout = task_fanout(1, available, Some(summary))?;
    object.insert(
        "task_fanout_default".into(),
        Value::from(single_scope_fanout),
    );
    object.insert(
        "single_scope_fanout".into(),
        Value::from(single_scope_fanout),
    );
    let mut examples = Vec::new();
    for scopes in [1_i64, 2, 3, 5, 8] {
        examples.push(serde_json::json!({
            "independent_scopes": scopes,
            "fanout": task_fanout(scopes, available, Some(summary))?,
        }));
    }
    object.insert("task_fanout_examples".into(), Value::Array(examples));
    // Example only — not a launch floor. Three independent scopes, capped by
    // available workers.
    let multi_scope_target = 3_i64.min(available.max(1));
    object.insert(
        "multi_scope_example_fanout".into(),
        Value::from(task_fanout(multi_scope_target, available, Some(summary))?),
    );
    Ok(())
}

fn insert_diversity_contract(
    object: &mut Map<String, Value>,
    settings: &Map<String, Value>,
    model_kinds: i64,
) {
    object.insert("available_model_kinds".into(), Value::from(model_kinds));
    let minimum_kinds = settings["minimum_model_kinds"].as_i64().unwrap_or(1);
    object.insert(
        "model_diversity_satisfied".into(),
        Value::Bool(model_kinds >= minimum_kinds),
    );
}

fn insert_policy_contract(
    object: &mut Map<String, Value>,
    summary: &Value,
    settings: &Map<String, Value>,
    available: i64,
) -> Result<()> {
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
        memory_management_contract(summary, settings)?,
    );
    object.insert(
        "automatic_selection_excluded_models".into(),
        Value::Array(sorted_excluded_models(summary)),
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
    Ok(())
}

fn sorted_excluded_models(summary: &Value) -> Vec<Value> {
    let mut excluded = summary
        .get("automatic_selection_excluded_models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    excluded.sort();
    excluded.into_iter().map(Value::from).collect()
}
