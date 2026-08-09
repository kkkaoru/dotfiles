use serde::Deserialize;
use serde_json::Value;

use super::request_routing::official_claude_haiku_model;
use super::subscription::valid_effort;

mod explicit;
use explicit::explicit_model_matches_agent;

const ADAPTER_EFFORT: &str = "claudex_effort";
const ADAPTER_MODEL: &str = "claudex_model";
const IMPLICIT_MODEL: &str = "claudex_implicit_model";

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
        return;
    }
}

fn configured_worker_fields(
    arguments: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
) -> Option<(String, Option<String>)> {
    let (model, effort) = model_catalog.worker_fields(arguments.get("subagent_type")?.as_str()?)?;
    Some((model.to_owned(), Some(effort.to_owned())))
}

fn generic_worker_fields(
    arguments: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
) -> Option<(String, Option<String>)> {
    if arguments.get(ADAPTER_MODEL).is_some()
        || !is_generic_agent_type(arguments.get("subagent_type").and_then(Value::as_str))
    {
        return None;
    }
    model_catalog
        .default_worker_fields()
        .map(|(model, effort)| (model.to_owned(), Some(effort.to_owned())))
}

fn generic_worker_model_matches(
    arguments: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
    model: &str,
) -> bool {
    is_generic_agent_type(arguments.get("subagent_type").and_then(Value::as_str))
        && model_catalog
            .default_worker_fields()
            .is_some_and(|(configured, _)| configured == model)
}

fn is_generic_agent_type(subagent_type: Option<&str>) -> bool {
    subagent_type.is_none()
        || subagent_type
            .map(str::trim)
            .is_some_and(|agent| matches!(agent, "Explore" | "general-purpose"))
}

fn selected_worker_fields(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
) -> Option<(String, Option<String>)> {
    let summary = active_routing_summary(messages, system)?;
    let workers = summary.get("selected_workers")?.as_array()?;
    let agent = arguments.get("subagent_type").and_then(Value::as_str);
    let worker = agent
        .and_then(|agent| {
            workers
                .iter()
                .find(|worker| worker.get("agent").and_then(Value::as_str) == Some(agent))
        })
        .or_else(|| {
            is_generic_agent_type(agent)
                .then(|| default_subagent_worker(&summary, workers))
                .flatten()
        })?;
    Some((
        worker.get("model")?.as_str()?.to_owned(),
        worker
            .get("effort")
            .and_then(Value::as_str)
            .filter(|effort| valid_effort(effort))
            .map(str::to_owned),
    ))
}

fn default_subagent_worker<'a>(summary: &'a Value, workers: &'a [Value]) -> Option<&'a Value> {
    let route = summary.get("default_subagent_route");
    route
        .and_then(|route| {
            let route_agent = route.get("agent").and_then(Value::as_str)?;
            let route_model = route.get("model").and_then(Value::as_str)?;
            workers.iter().find(|worker| {
                worker.get("agent").and_then(Value::as_str) == Some(route_agent)
                    && worker.get("model").and_then(Value::as_str) == Some(route_model)
            })
        })
        .or_else(|| {
            workers
                .iter()
                .find(|worker| worker.get("model").and_then(Value::as_str).is_some())
        })
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

pub(super) fn model_is_authorized(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
    model: &str,
) -> bool {
    // Claude Code can preserve its built-in `general-purpose` / `Explore` type while passing a
    // routed provider model. The selected model remains the authority; requiring its display
    // type to equal the configured worker name rejects that valid launch before ACP can start.
    if advisor_launch_disabled(arguments, messages, system) {
        return false;
    }
    explicit::active_user_requests_model(messages, model)
        || selected_worker_model_matches(arguments, messages, system, model)
        || configured_advisor_model_matches(arguments, messages, system, model)
}

pub(super) fn model_is_authorized_with_catalog(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
    model: &str,
) -> bool {
    if advisor_launch_disabled(arguments, messages, system) {
        return false;
    }
    if let Some((expected, _)) = expected_worker_fields(arguments, messages, system, model_catalog)
    {
        return expected == model;
    }
    arguments.get(IMPLICIT_MODEL).and_then(Value::as_str) == Some(model)
        || model_is_authorized(arguments, messages, system, model)
        || generic_worker_model_matches(arguments, model_catalog, model)
}

pub(super) fn routing_disables_subagent_model(
    messages: &[Value],
    system: &Value,
    model: &str,
) -> bool {
    active_routing_summary(messages, system).is_some_and(|summary| {
        summary
            .get("disabled_subagent_models")
            .and_then(Value::as_array)
            .is_some_and(|models| models.iter().any(|value| value.as_str() == Some(model)))
    })
}

fn selected_worker_model_matches(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
    model: &str,
) -> bool {
    selected_worker_fields(arguments, messages, system)
        .is_some_and(|(selected, _)| selected == model)
}

fn advisor_launch_disabled(arguments: &Value, messages: &[Value], system: &Value) -> bool {
    let Some(agent) = arguments.get("subagent_type").and_then(Value::as_str) else {
        return false;
    };
    active_routing_summary(messages, system).is_some_and(|summary| {
        summary
            .get("custom_advisor_enabled")
            .and_then(Value::as_bool)
            .is_some_and(|enabled| {
                !enabled
                    && summary
                        .get("advisor")
                        .and_then(|advisor| advisor.get("agent"))
                        .and_then(Value::as_str)
                        == Some(agent)
            })
    })
}

fn configured_advisor_model_matches(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
    model: &str,
) -> bool {
    let Some(agent) = arguments.get("subagent_type").and_then(Value::as_str) else {
        return false;
    };
    active_routing_summary(messages, system).is_some_and(|summary| {
        summary
            .get("custom_advisor_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true)
            && summary.get("advisor").is_some_and(|advisor| {
                advisor.get("agent").and_then(Value::as_str) == Some(agent)
                    && advisor.get("model").and_then(Value::as_str) == Some(model)
            })
    })
}

fn active_routing_summary(messages: &[Value], system: &Value) -> Option<Value> {
    // The hook normally places the current snapshot in a user message, but Claude Code can
    // retain it in an assistant/tool transcript after compaction or a resumed turn. Prefer the
    // request-level system snapshot, then the latest user snapshot, and finally any transcript
    // snapshot so an otherwise valid routed worker is not rejected after context reshaping.
    value_texts(system)
        .filter_map(routing_summary)
        .last()
        .or_else(|| {
            user_message_texts(messages)
                .filter_map(routing_summary)
                .last()
        })
        .or_else(|| message_texts(messages).filter_map(routing_summary).last())
}

fn routing_summary(text: &str) -> Option<Value> {
    let start = text.find("{\"providers\":")?;
    Value::deserialize(&mut serde_json::Deserializer::from_str(&text[start..])).ok()
}

fn user_message_texts(messages: &[Value]) -> impl Iterator<Item = &str> {
    messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content"))
        .flat_map(value_texts)
}

fn message_texts(messages: &[Value]) -> impl Iterator<Item = &str> {
    messages
        .iter()
        .filter_map(|message| message.get("content"))
        .flat_map(value_texts)
}

fn value_texts(value: &Value) -> impl Iterator<Item = &str> {
    value.as_str().into_iter().chain(
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| block.get("text").and_then(Value::as_str)),
    )
}

#[cfg(test)]
mod tests {
    use super::{ADAPTER_MODEL, IMPLICIT_MODEL, hydrate_standard_agent_to_parent};

    #[test]
    fn standard_agent_without_type_does_not_inherit_parent_model() {
        let mut arguments = serde_json::json!({"prompt": "inspect the current state"});

        hydrate_standard_agent_to_parent(&mut arguments, "gpt-5.6-luna");

        assert!(arguments.get(ADAPTER_MODEL).is_none());
        assert!(arguments.get(IMPLICIT_MODEL).is_none());
    }

    #[test]
    fn configured_worker_type_without_model_is_not_guessed() {
        let mut arguments = serde_json::json!({
            "subagent_type": "claudex-gpt",
            "prompt": "inspect the current state"
        });

        hydrate_standard_agent_to_parent(&mut arguments, "gpt-5.6-luna");

        assert!(arguments.get(ADAPTER_MODEL).is_none());
        assert!(arguments.get(IMPLICIT_MODEL).is_none());
    }
}
