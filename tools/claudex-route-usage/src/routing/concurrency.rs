//! Daemon concurrency refresh and re-ranking of the selected worker pool.

use super::orchestration::orchestration_contract;
use super::quota::combined_capacity_priority_with_inflight;
use super::workers::{
    extend_with_native_workers, fallback_worker, native_worker_item, prefer_weekly_headroom,
    ranked_native_quota, selected_agent_values, worker,
};
use super::{CapacityKey, rank_selected_workers};
use crate::config::Config;
use anyhow::Result;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

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

/// The unlimited concurrency status shared by native and unmetered workers.
fn not_limited() -> Value {
    concurrency_status(None, None, None, true, "not-limited", true)
}

pub fn provider_for_model<'a>(config: &'a Config, model: &str) -> Option<&'a Value> {
    let exact = config.providers.iter().find(|provider| {
        let default = provider.get("defaultModel").and_then(Value::as_str);
        let subagent = provider
            .get("subagentModel")
            .and_then(Value::as_str)
            .or(default);
        default == Some(model) || subagent == Some(model)
    });
    if let Some(exact) = exact {
        return Some(exact);
    }
    let mut matches: Vec<(usize, i64, &Value)> = config
        .providers
        .iter()
        .enumerate()
        .flat_map(|(index, provider)| prefix_matches(provider, model, index as i64))
        .collect();
    matches.sort_by(|left, right| right.0.cmp(&left.0).then(right.1.cmp(&left.1)));
    matches.first().map(|item| item.2)
}

/// Longest-prefix candidates for one provider, keyed for descending sort.
fn prefix_matches<'a>(
    provider: &'a Value,
    model: &str,
    index: i64,
) -> Vec<(usize, i64, &'a Value)> {
    provider
        .get("modelPrefixes")
        .and_then(Value::as_array)
        .map(|prefixes| {
            prefixes
                .iter()
                .filter_map(Value::as_str)
                .filter(|prefix| model.starts_with(prefix))
                .map(|prefix| (prefix.len(), -index, provider))
                .collect()
        })
        .unwrap_or_default()
}

pub fn model_concurrency_status(
    provider: &Value,
    model: &str,
    health: Option<&BTreeMap<String, Value>>,
) -> Value {
    let configured_limit = provider.get("maxConcurrency").and_then(Value::as_i64);
    let Some(configured_limit) = configured_limit else {
        return not_limited();
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
    let active = slot_count(fields, "active");
    let queued = slot_count(fields, "queued");
    if slot_count(fields, "limit") != configured_limit {
        return concurrency_status(
            Some(active),
            Some(queued),
            Some(configured_limit),
            active + queued < configured_limit,
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

fn slot_count(fields: &Value, key: &str) -> i64 {
    fields.get(key).and_then(Value::as_i64).unwrap_or_default()
}

/// One provider's inputs while refreshing concurrency for the summary.
struct ProviderSlot<'a> {
    provider: &'a Value,
    index: i64,
    health: Option<&'a BTreeMap<String, Value>>,
    active_subagent_models: &'a BTreeMap<String, i64>,
    disabled_models: &'a BTreeSet<String>,
}

pub fn apply_model_concurrency_with_inflight(
    summary: Value,
    config: &Config,
    health: Option<&BTreeMap<String, Value>>,
    active_subagent_models: &BTreeMap<String, i64>,
    disabled_models: &BTreeSet<String>,
) -> Result<Value> {
    let mut combined = summary;
    let mut automatic_candidates: Vec<(CapacityKey, Value)> = Vec::new();
    let mut model_capacity = Map::new();
    for (index, provider) in config.providers.iter().enumerate() {
        refresh_provider_slot(
            &mut combined,
            &mut automatic_candidates,
            &mut model_capacity,
            &ProviderSlot {
                provider,
                index: index as i64,
                health,
                active_subagent_models,
                disabled_models,
            },
        );
    }
    record_health_only_models(&mut model_capacity, config, health);
    let providers = combined
        .get("providers")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let native_quota = combined
        .get("native_worker_quota")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let participating = collect_native_candidates(
        &combined,
        config,
        disabled_models,
        active_subagent_models,
        &mut automatic_candidates,
    );
    let mut selected = rank_selected_workers(automatic_candidates);
    selected = prefer_weekly_headroom(selected, &providers, &native_quota);
    let fallback = fallback_worker(config);
    let fallback_model = fallback
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let fallback_active = selected.is_empty() && !disabled_models.contains(fallback_model);
    if fallback_active {
        selected = vec![fallback];
    }
    extend_with_native_workers(&mut selected, config, disabled_models, &participating);
    write_selection(&mut combined, selected, fallback_active, model_capacity);
    let orchestration = orchestration_contract(&combined)?;
    if let Some(object) = combined.as_object_mut() {
        object.insert("orchestration".into(), orchestration);
    }
    Ok(combined)
}

/// Refresh one provider's concurrency fields and rank it when it has capacity.
fn refresh_provider_slot(
    combined: &mut Value,
    candidates: &mut Vec<(CapacityKey, Value)>,
    model_capacity: &mut Map<String, Value>,
    slot: &ProviderSlot<'_>,
) {
    let provider_id = slot
        .provider
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(fields) = combined
        .get_mut("providers")
        .and_then(Value::as_object_mut)
        .and_then(|providers| providers.get_mut(provider_id))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let current_worker = worker(slot.provider);
    let model = current_worker
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let concurrency = model_concurrency_status(slot.provider, &model, slot.health);
    let limited = slot.provider.get("maxConcurrency").is_some();
    if limited {
        model_capacity.insert(model.clone(), concurrency.clone());
        insert_concurrency_fields(fields, &concurrency);
    }
    let quota = quota_snapshot(fields);
    let disabled = slot.disabled_models.contains(&model)
        || fields
            .get("disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if disabled {
        return;
    }
    if !flag(&quota, "available") {
        return;
    }
    if !flag(&concurrency, "available") {
        fields.insert("available".into(), Value::Bool(false));
        fields.insert("reason".into(), Value::from("concurrency-limit-reached"));
        return;
    }
    let mut selected_worker = current_worker;
    inject_worker_concurrency(&mut selected_worker, &concurrency, limited);
    candidates.push((
        combined_capacity_priority_with_inflight(
            &quota,
            &concurrency,
            slot.index,
            *slot.active_subagent_models.get(&model).unwrap_or(&0),
        ),
        selected_worker,
    ));
}

fn inject_worker_concurrency(worker: &mut Value, concurrency: &Value, limited: bool) {
    if !limited {
        return;
    }
    let Some(object) = worker.as_object_mut() else {
        return;
    };
    object.insert("concurrency".into(), concurrency.clone());
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn insert_concurrency_fields(fields: &mut Map<String, Value>, concurrency: &Value) {
    for (target, source) in [
        ("concurrency_active", "active"),
        ("concurrency_queued", "queued"),
        ("concurrency_limit", "limit"),
        ("concurrency_remaining", "remaining"),
        ("concurrency_available", "available"),
        ("concurrency_reason", "reason"),
    ] {
        fields.insert(target.to_owned(), concurrency[source].clone());
    }
}

fn quota_snapshot(fields: &Map<String, Value>) -> Value {
    serde_json::json!({
        "available": fields.get("available").cloned().unwrap_or(Value::Bool(false)),
        "max_used_percent": fields.get("max_used_percent").cloned().unwrap_or(Value::Null),
        "reason": fields.get("reason").cloned().unwrap_or(Value::Null),
        "quota_windows": fields.get("quota_windows").cloned().unwrap_or_else(|| Value::Object(Map::new())),
    })
}

/// Publish capacity for daemon-reported models that no selected worker uses.
fn record_health_only_models(
    model_capacity: &mut Map<String, Value>,
    config: &Config,
    health: Option<&BTreeMap<String, Value>>,
) {
    let Some(health) = health else {
        return;
    };
    for model in health.keys() {
        if let Some(provider) = provider_for_model(config, model)
            && provider.get("maxConcurrency").is_some()
            && !shadowed_default_model(provider, model)
        {
            model_capacity.insert(
                model.clone(),
                model_concurrency_status(provider, model, Some(health)),
            );
        }
    }
}

/// A provider default model that its subagent worker overrides is not routable.
fn shadowed_default_model(provider: &Value, model: &str) -> bool {
    let worker_item = worker(provider);
    let worker_model = worker_item
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let default_model = provider
        .get("defaultModel")
        .and_then(Value::as_str)
        .unwrap_or_default();
    model == default_model && model != worker_model
}

fn collect_native_candidates(
    combined: &Value,
    config: &Config,
    disabled_models: &BTreeSet<String>,
    active_subagent_models: &BTreeMap<String, i64>,
    candidates: &mut Vec<(CapacityKey, Value)>,
) -> BTreeSet<String> {
    let native_quota = combined
        .get("native_worker_quota")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut participating = BTreeSet::new();
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
        let Some(quota) = native_quota
            .get(agent)
            .filter(|quota| ranked_native_quota(quota))
        else {
            continue;
        };
        participating.insert(agent.to_owned());
        candidates.push((
            combined_capacity_priority_with_inflight(
                quota,
                &not_limited(),
                (config.providers.len() + native_index) as i64,
                *active_subagent_models.get(model).unwrap_or(&0),
            ),
            native_item,
        ));
    }
    participating
}

fn write_selection(
    combined: &mut Value,
    selected: Vec<Value>,
    fallback_active: bool,
    model_capacity: Map<String, Value>,
) {
    let preferred = selected.first().cloned();
    let Some(object) = combined.as_object_mut() else {
        return;
    };
    object.insert("selected_agents".into(), selected_agent_values(&selected));
    object.insert("selected_workers".into(), Value::Array(selected));
    object.insert("preferred_worker".into(), preferred.unwrap_or(Value::Null));
    object.insert("fallback_active".into(), Value::Bool(fallback_active));
    object.insert("model_concurrency".into(), Value::Object(model_capacity));
}
