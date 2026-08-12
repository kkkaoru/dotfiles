//! Worker items, capacity metadata, and main/worker model separation.

use super::orchestration::orchestration_contract;
use super::quota::{effective_window_remaining, selection_remaining};
use crate::config::Config;
use crate::util::{copied_fields, is_sonnet_model, number_f64, python_round};
use anyhow::Result;
use serde_json::{Map, Value};
use std::collections::BTreeSet;

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
    let mut item = serde_json::json!({
        "provider": "native",
        "agent": worker_cfg.get("agent").and_then(Value::as_str).unwrap_or_default(),
        "model": worker_cfg.get("model").and_then(Value::as_str).unwrap_or_default(),
        "effort": worker_cfg.get("effort").and_then(Value::as_str).unwrap_or_default(),
    });
    if let Some(usage_provider) = worker_cfg.get("usageProvider").and_then(Value::as_str) {
        item["usage_provider"] = Value::from(usage_provider);
    }
    item
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

/// The subscription fallback worker, tagged with its synthetic provider id.
pub fn fallback_worker(config: &Config) -> Value {
    let mut fallback = config.fallback.clone();
    if let Some(object) = fallback.as_object_mut() {
        object.insert("provider".into(), Value::from("fallback"));
    }
    fallback
}

pub fn selected_agent_values(selected: &[Value]) -> Value {
    Value::Array(
        selected
            .iter()
            .filter_map(|item| item.get("agent").cloned())
            .collect(),
    )
}

/// Append native workers that are neither already selected nor ranked by capacity.
pub fn extend_with_native_workers(
    selected: &mut Vec<Value>,
    config: &Config,
    disabled_models: &BTreeSet<String>,
    participating: &BTreeSet<String>,
) {
    let mut existing_agents = participating.clone();
    existing_agents.extend(
        selected
            .iter()
            .filter_map(|item| item.get("agent").and_then(Value::as_str))
            .map(str::to_owned),
    );
    selected.extend(
        selected_native_workers(config, disabled_models)
            .into_iter()
            .filter(|item| {
                item.get("agent")
                    .and_then(Value::as_str)
                    .is_none_or(|agent| !existing_agents.contains(agent))
            }),
    );
}

/// Weekly remaining below this is treated as depleted for automatic picks.
pub const LOW_WEEKLY_REMAINING_PERCENT: f64 = 25.0;
/// At least one worker at or above this enables depleting low-weekly peers.
pub const AMPLE_WEEKLY_REMAINING_PERCENT: f64 = 40.0;
type WorkerHeadroom = (Value, Option<f64>, Option<f64>, Option<f64>);
/// Prefer high quota headroom for automatic SubAgent selection.
///
/// When any selected worker has ample selection remaining, drop peers whose
/// selection remaining is low **or unknown**. Selection remaining is weekly when
/// only weekly is known, and `min(weekly, five-hour)` when a five-hour meter is
/// present. Unknown meters (for example Ollama `available-ollama-api-only`) must
/// not stay in the automatic pool beside peers with real headroom — reachability
/// is not quota. Intentional unmetered workers (no `usageProvider`) stay in the
/// automatic pool, ranked behind known weekly meters. Explicit model launches
/// can still target a dropped provider via `model_prefixes` when the active user
/// names that model.
pub fn prefer_weekly_headroom(
    selected: Vec<Value>,
    providers: &Map<String, Value>,
    native_quota: &Map<String, Value>,
) -> Vec<Value> {
    let annotated: Vec<WorkerHeadroom> = selected
        .into_iter()
        .map(|worker_item| {
            let (weekly, five_hour) = worker_item
                .as_object()
                .and_then(|object| worker_quota(object, providers, native_quota))
                .map(effective_window_remaining)
                .unwrap_or((None, None));
            let remaining = selection_remaining(weekly, five_hour);
            (worker_item, weekly, five_hour, remaining)
        })
        .collect();
    let has_ample = annotated.iter().any(|(_, _, _, remaining)| {
        remaining.is_some_and(|value| value >= AMPLE_WEEKLY_REMAINING_PERCENT)
    });
    let keep_all = !has_ample;
    let mut filtered = Vec::with_capacity(annotated.len());
    for (mut worker_item, weekly, five_hour, remaining) in annotated {
        let unmetered = worker_item.as_object().is_some_and(|object| {
            worker_quota(object, providers, native_quota)
                .and_then(|quota| quota.get("reason"))
                .and_then(Value::as_str)
                == Some("unmetered")
        });
        if !keep_all
            && remaining.is_none_or(|value| value < LOW_WEEKLY_REMAINING_PERCENT)
            && !unmetered
        {
            continue;
        }
        if let Some(object) = worker_item.as_object_mut() {
            object.insert(
                "weekly_remaining_percent".into(),
                weekly.map_or(Value::Null, |value| Value::from(python_round(value, 1))),
            );
            object.insert(
                "five_hour_remaining_percent".into(),
                five_hour.map_or(Value::Null, |value| Value::from(python_round(value, 1))),
            );
        }
        filtered.push(worker_item);
    }
    filtered
}

/// Native workers only join the ranking when CodexBar reported real usage.
pub fn ranked_native_quota(quota: &Value) -> bool {
    quota
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && quota.get("max_used_percent").and_then(number_f64).is_some()
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
    let workers = summary
        .get("selected_workers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    workers
        .iter()
        .filter_map(|worker_item| capacity_entry(worker_item, &provider_status, &native_quota))
        .collect()
}

fn capacity_entry(
    worker_item: &Value,
    provider_status: &Map<String, Value>,
    native_quota: &Map<String, Value>,
) -> Option<Value> {
    let object = worker_item.as_object()?;
    let mut entry = copied_fields(object, &["agent", "model"]);
    let quota = worker_quota(object, provider_status, native_quota);
    if let Some(usage_provider) = object.get("usage_provider") {
        entry.insert("usage_provider".into(), usage_provider.clone());
    }
    insert_used_percent(&mut entry, quota);
    let (weekly, five_hour) =
        effective_window_remaining(quota.unwrap_or(&Value::Object(Map::new())));
    entry.insert(
        "weekly_remaining_percent".into(),
        rounded_percent(weekly, 1),
    );
    entry.insert(
        "five_hour_remaining_percent".into(),
        rounded_percent(five_hour, 1),
    );
    Some(Value::Object(entry))
}

fn worker_quota<'a>(
    worker_item: &Map<String, Value>,
    provider_status: &'a Map<String, Value>,
    native_quota: &'a Map<String, Value>,
) -> Option<&'a Value> {
    let quota = worker_item
        .get("provider")
        .and_then(Value::as_str)
        .and_then(|provider| provider_status.get(provider));
    // Native Claude workers (haiku-search / sonnet) and the empty-pool
    // fallback share agent-keyed quota from CodexBar's claude provider.
    quota.or_else(|| {
        worker_item
            .get("agent")
            .and_then(Value::as_str)
            .and_then(|agent| native_quota.get(agent))
    })
}

fn insert_used_percent(entry: &mut Map<String, Value>, quota: Option<&Value>) {
    let used = quota
        .filter(|value| value.get("max_used_percent").and_then(number_f64).is_some())
        .and_then(|value| number_f64(&value["max_used_percent"]));
    let Some(used) = used else {
        entry.insert("used_percent".into(), Value::Null);
        entry.insert("remaining_percent".into(), Value::Null);
        return;
    };
    entry.insert("used_percent".into(), Value::from(used));
    entry.insert(
        "remaining_percent".into(),
        Value::from(python_round(100.0 - used, 1)),
    );
}

fn rounded_percent(value: Option<f64>, ndigits: i32) -> Value {
    value.map_or(Value::Null, |percent| {
        Value::from(python_round(percent, ndigits))
    })
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
    let mut route = copied_fields(top, &["agent", "model", "effort"]);
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

/// Resolved main/worker separation policy for one routing summary.
struct Separation<'a> {
    current_main_model: Option<&'a str>,
    current_main_model_known: bool,
    excluded_models: BTreeSet<String>,
    sonnet_suppressed: bool,
    allow_sonnet_subagent: bool,
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
    let (selected, excluded_models, sonnet_suppressed) =
        if is_sonnet_model(current_main_model) && !allow_sonnet_subagent {
            suppress_sonnet_workers(selected)
        } else {
            (selected, BTreeSet::new(), false)
        };
    write_separation_fields(
        &mut separated,
        &selected,
        Separation {
            current_main_model,
            current_main_model_known: main_model_known && current_main_model.is_some(),
            excluded_models,
            sonnet_suppressed,
            allow_sonnet_subagent,
        },
    );
    let orchestration = orchestration_contract(&separated)?;
    let delegation_required = !selected.is_empty()
        && orchestration
            .get("subagent_first")
            .and_then(Value::as_bool)
            .unwrap_or(true);
    write_delegation_fields(&mut separated, orchestration, delegation_required);
    let orchestration = orchestration_contract(&separated)?;
    if let Some(object) = separated.as_object_mut() {
        object.insert("orchestration".into(), orchestration);
    }
    Ok(separated)
}

/// Drop Sonnet workers so a Sonnet main session never pays twice for the
/// identical subscription request, reporting the excluded model ids.
fn suppress_sonnet_workers(selected: Vec<Value>) -> (Vec<Value>, BTreeSet<String>, bool) {
    let mut retained = Vec::new();
    let mut excluded = BTreeSet::new();
    let mut suppressed = false;
    for worker_item in selected {
        let model = worker_item.get("model").and_then(Value::as_str);
        if !is_sonnet_model(model) {
            retained.push(worker_item);
            continue;
        }
        excluded.extend(model.map(str::to_owned));
        suppressed = true;
    }
    (retained, excluded, suppressed)
}

fn write_separation_fields(separated: &mut Value, selected: &[Value], policy: Separation<'_>) {
    let preferred = selected.first().cloned();
    let Some(object) = separated.as_object_mut() else {
        return;
    };
    object.insert("selected_agents".into(), selected_agent_values(selected));
    object.insert("selected_workers".into(), Value::Array(selected.to_vec()));
    object.insert("preferred_worker".into(), preferred.unwrap_or(Value::Null));
    let main_model = policy.current_main_model.map_or(Value::Null, Value::from);
    object.insert("current_main_model".into(), main_model.clone());
    object.insert(
        "current_main_model_known".into(),
        Value::Bool(policy.current_main_model_known),
    );
    object.insert("main_session_model".into(), main_model);
    object.insert(
        "automatic_selection_excluded_models".into(),
        Value::Array(
            policy
                .excluded_models
                .into_iter()
                .map(Value::from)
                .collect(),
        ),
    );
    object.insert(
        "sonnet_subagent_suppressed".into(),
        Value::Bool(policy.sonnet_suppressed),
    );
    object.insert(
        "sonnet_subagent_explicit_allowed".into(),
        Value::Bool(policy.allow_sonnet_subagent),
    );
    if policy.sonnet_suppressed {
        object.insert("fallback_active".into(), Value::Bool(false));
    }
    object.insert("orchestration_mode".into(), Value::from("subagent-first"));
}

fn write_delegation_fields(separated: &mut Value, orchestration: Value, delegation_required: bool) {
    let Some(object) = separated.as_object_mut() else {
        return;
    };
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
