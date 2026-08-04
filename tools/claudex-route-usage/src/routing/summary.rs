//! Routing summary assembly from a CodexBar usage report.

use super::orchestration::orchestration_contract;
use super::quota::{capacity_priority, quota_with_windows, status};
use super::workers::{
    extend_with_native_workers, fallback_worker, native_worker_item, ranked_native_quota,
    selected_agent_values, selected_native_workers, worker,
};
use super::{CapacityKey, rank_selected_workers};
use crate::config::Config;
use anyhow::Result;
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub fn routing_summary(
    report: &Value,
    config: &Config,
    disabled_models: &BTreeSet<String>,
) -> Result<Value> {
    let mut candidates: Vec<(CapacityKey, Value)> = Vec::new();
    let providers = provider_quota_fields(report, config, disabled_models, &mut candidates)?;
    let native_quota = native_quota_fields(report, config, disabled_models, &mut candidates)?;
    let mut selected = rank_selected_workers(candidates);
    let fallback_active = selected.is_empty()
        && config
            .fallback
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|model| !disabled_models.contains(model));
    if fallback_active {
        selected = vec![fallback_worker(config)];
    }
    let participating: BTreeSet<String> = selected
        .iter()
        .filter_map(|item| item.get("agent").and_then(Value::as_str).map(str::to_owned))
        .collect();
    extend_with_native_workers(&mut selected, config, disabled_models, &participating);
    let preferred = selected.first().cloned();
    let mut summary = serde_json::json!({
        "providers": providers,
        "native_worker_quota": native_quota,
        "selected_agents": selected_agent_values(&selected),
        "selected_workers": selected,
        "preferred_worker": preferred,
        "fallback_active": fallback_active,
        "disabled_subagent_models": disabled_models.iter().cloned().collect::<Vec<_>>(),
        "advisor": config.advisor.clone(),
    });
    summary["orchestration"] = orchestration_contract(&summary)?;
    Ok(summary)
}

/// Per-provider quota fields, also ranking every provider with capacity.
fn provider_quota_fields(
    report: &Value,
    config: &Config,
    disabled_models: &BTreeSet<String>,
    candidates: &mut Vec<(CapacityKey, Value)>,
) -> Result<Map<String, Value>> {
    let mut providers = Map::new();
    for (index, provider) in config.providers.iter().enumerate() {
        let quota = quota_with_windows(report, provider, false)?;
        let worker_item = worker(provider);
        let disabled = worker_item
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|model| disabled_models.contains(model));
        let effective = if disabled {
            status(false, None, "disabled-by-policy")
        } else {
            quota.clone()
        };
        let provider_id = provider
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        providers.insert(
            provider_id.to_owned(),
            Value::Object(provider_fields(&effective, &worker_item, disabled)),
        );
        if !disabled
            && quota
                .get("available")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            candidates.push((capacity_priority(&quota, index as i64), worker_item));
        }
    }
    Ok(providers)
}

/// Merge one quota status with its worker item for the `providers` map.
fn provider_fields(quota: &Value, worker_item: &Value, disabled: bool) -> Map<String, Value> {
    let mut fields = quota.as_object().cloned().unwrap_or_default();
    if let Some(object) = worker_item.as_object() {
        fields.extend(object.clone());
    }
    fields.insert("disabled".into(), Value::Bool(disabled));
    fields
}

/// Agent-keyed quota for native Claude workers, ranking those with real usage.
fn native_quota_fields(
    report: &Value,
    config: &Config,
    disabled_models: &BTreeSet<String>,
    candidates: &mut Vec<(CapacityKey, Value)>,
) -> Result<Map<String, Value>> {
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
        let quota = quota_with_windows(report, native, true)?;
        let agent = native
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or_default();
        native_quota.insert(agent.to_owned(), quota.clone());
        if ranked_native_quota(&quota) {
            candidates.push((
                capacity_priority(&quota, (config.providers.len() + native_index) as i64),
                native_item,
            ));
        }
    }
    Ok(native_quota)
}

pub fn fallback_summary(
    reason: &str,
    config: &Config,
    disabled_models: &BTreeSet<String>,
) -> Result<Value> {
    let mut providers = Map::new();
    for provider in &config.providers {
        let worker_item = worker(provider);
        let disabled = worker_item
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|model| disabled_models.contains(model));
        let unavailable_reason = if disabled {
            "disabled-by-policy"
        } else {
            reason
        };
        let provider_id = provider
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        providers.insert(
            provider_id.to_owned(),
            Value::Object(provider_fields(
                &status(false, None, unavailable_reason),
                &worker_item,
                disabled,
            )),
        );
    }
    let fallback = fallback_worker(config);
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
        "selected_agents": selected_agent_values(&selected),
        "selected_workers": selected,
        "preferred_worker": preferred,
        "fallback_active": fallback_active,
        "disabled_subagent_models": disabled_models.iter().cloned().collect::<Vec<_>>(),
        "advisor": config.advisor.clone(),
    });
    summary["orchestration"] = orchestration_contract(&summary)?;
    Ok(summary)
}
