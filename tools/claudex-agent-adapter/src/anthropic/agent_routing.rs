use serde_json::Value;

use super::request_routing::official_claude_haiku_model;
use super::subscription::valid_effort;

mod authorize;
mod explicit;
mod summary;
mod texts;
mod workers;
pub(in crate::anthropic) use authorize::{
    model_is_authorized, model_is_authorized_with_catalog, routing_disables_subagent_model,
};
use explicit::explicit_model_matches_agent;
pub(in crate::anthropic) use summary::active_routing_summary;
use texts::user_message_texts;
use workers::{
    configured_worker_fields, generic_worker_fields, is_generic_agent_type, selected_worker_fields,
};

const ADAPTER_EFFORT: &str = "claudex_effort";
pub(super) const ADAPTER_MODEL: &str = "claudex_model";
pub(super) const IMPLICIT_MODEL: &str = "claudex_implicit_model";

pub(super) fn hydrate_routing_fields(arguments: &mut Value) {
    // Only explicit claudex_model fields / prompt headers are trusted. Do not infer from the
    // Claude Code `model` field via vendor name prefixes; that becomes config debt.
    let model = arguments
        .get(ADAPTER_MODEL)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| prompt_routing_value(arguments, ADAPTER_MODEL));
    let effort = arguments
        .get(ADAPTER_EFFORT)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| prompt_routing_value(arguments, ADAPTER_EFFORT));
    let Some(object) = arguments.as_object_mut() else {
        return;
    };
    if let Some(model) = model {
        object.insert(ADAPTER_MODEL.to_owned(), Value::String(model));
    }
    if let Some(effort) = effort.filter(|value| valid_effort(value)) {
        object.insert(ADAPTER_EFFORT.to_owned(), Value::String(effort));
    }
}

/// Native Claude subscription tools do not expose Claudex-only schema fields.
/// Recover them only from the exact selected worker named by the tool call.
pub(super) fn hydrate_routing_fields_from_context(
    arguments: &mut Value,
    messages: &[Value],
    system: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
) {
    // This marker is adapter-owned. Never let an Agent/Task caller authorize its
    // own arbitrary model by supplying the private field.
    if let Some(object) = arguments.as_object_mut() {
        object.remove(IMPLICIT_MODEL);
    }
    hydrate_routing_fields(arguments);
    let Some((model, effort)) = expected_worker_fields(arguments, messages, system, model_catalog)
    else {
        return;
    };
    let Some(object) = arguments.as_object_mut() else {
        return;
    };
    // Model and effort are one route tuple. Always canonicalize both together;
    // retaining either caller-provided half can combine two different workers.
    object.insert(ADAPTER_MODEL.to_owned(), Value::String(model));
    if let Some(effort) = effort {
        object.insert(ADAPTER_EFFORT.to_owned(), Value::String(effort));
    } else {
        object.remove(ADAPTER_EFFORT);
    }
}

pub(super) fn expected_worker_fields(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
) -> Option<(String, Option<String>)> {
    // Generic Agent types stay bound to live selected_workers. A named catalog
    // worker such as `claudex-qwen` is an explicit launch: after Cline Credits
    // exhaustion the orchestrator reroutes to that sibling even when the stale
    // snapshot still lists only the depleted automatic pool. Adapter exhaustion
    // cooldown still blocks a truly spent provider at launch time.
    let routing_present = active_routing_summary(messages, system).is_some();
    let mut fields = selected_worker_fields(arguments, messages, system)
        .or_else(|| configured_worker_fields(arguments, model_catalog))
        .or_else(|| {
            (!routing_present)
                .then(|| generic_worker_fields(arguments, model_catalog))
                .flatten()
        })?;
    if let Some(requested) = arguments
        .get(ADAPTER_MODEL)
        .and_then(Value::as_str)
        .filter(|model| explicit_model_matches_agent(arguments, messages, system, model))
    {
        fields.0 = requested.to_owned();
    }
    Some(fields)
}

/// Route native Claude children to the official Haiku alias.
///
/// Generic Agent types are hydrated by `hydrate_routing_fields_from_context` from a selected or
/// configured Claudex worker. This function intentionally does not inherit the outer Claude
/// session model: if no Claudex route exists, validation must report the missing route.
pub(super) fn hydrate_standard_agent_to_parent(arguments: &mut Value, parent_model: &str) {
    let _ = parent_model;
    let subagent_type = arguments
        .get("subagent_type")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if subagent_type.as_deref() == Some("claude") && arguments.get(ADAPTER_MODEL).is_none() {
        let Some(object) = arguments.as_object_mut() else {
            return;
        };
        object.insert(
            ADAPTER_MODEL.to_owned(),
            Value::String(official_claude_haiku_model().to_owned()),
        );
        object.insert(
            IMPLICIT_MODEL.to_owned(),
            Value::String(official_claude_haiku_model().to_owned()),
        );
        object.insert(ADAPTER_EFFORT.to_owned(), Value::String("max".to_owned()));
    }
}

fn prompt_routing_value(arguments: &Value, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    arguments
        .get("prompt")
        .and_then(Value::as_str)?
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
#[path = "agent_routing_tests.rs"]
mod tests;
