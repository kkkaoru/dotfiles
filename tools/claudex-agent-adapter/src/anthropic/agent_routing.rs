use serde::Deserialize;
use serde_json::Value;

use super::request_routing::official_claude_haiku_model;
use super::subscription::valid_effort;

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
    hydrate_routing_fields(arguments);
    let Some((model, effort)) = selected_worker_fields(arguments, messages, system)
        .or_else(|| configured_worker_fields(arguments, model_catalog))
    else {
        return;
    };
    let Some(object) = arguments.as_object_mut() else {
        return;
    };
    object
        .entry(ADAPTER_MODEL.to_owned())
        .or_insert(Value::String(model));
    if let Some(effort) = effort {
        object
            .entry(ADAPTER_EFFORT.to_owned())
            .or_insert(Value::String(effort));
    }
}

/// Route a generic Claude child from the main session to the official Haiku fallback. A child
/// launched inside a routed SubAgent instead inherits that SubAgent's normalized model.
pub(super) fn hydrate_standard_agent_to_parent(
    arguments: &mut Value,
    parent_model: &str,
    inherit_parent_for_nested_child: bool,
) {
    let Some(subagent_type) = arguments
        .get("subagent_type")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return;
    };
    if subagent_type == "claude" && arguments.get(ADAPTER_MODEL).is_none() {
        let Some(object) = arguments.as_object_mut() else {
            return;
        };
        let model = if inherit_parent_for_nested_child && !parent_model.is_empty() {
            parent_model
        } else {
            official_claude_haiku_model()
        };
        object.insert(ADAPTER_MODEL.to_owned(), Value::String(model.to_owned()));
        object.insert(IMPLICIT_MODEL.to_owned(), Value::String(model.to_owned()));
        return;
    }
    if !matches!(subagent_type.as_str(), "Explore" | "general-purpose")
        || parent_model.is_empty()
        || subagent_type.starts_with("claudex-")
        || arguments.get(ADAPTER_MODEL).is_some()
    {
        return;
    }
    let Some(object) = arguments.as_object_mut() else {
        return;
    };
    object.insert(
        ADAPTER_MODEL.to_owned(),
        Value::String(parent_model.to_owned()),
    );
    object.insert(
        IMPLICIT_MODEL.to_owned(),
        Value::String(parent_model.to_owned()),
    );
}

fn configured_worker_fields(
    arguments: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
) -> Option<(String, Option<String>)> {
    let (model, effort) = model_catalog.worker_fields(arguments.get("subagent_type")?.as_str()?)?;
    Some((model.to_owned(), Some(effort.to_owned())))
}

fn selected_worker_fields(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
) -> Option<(String, Option<String>)> {
    let agent = arguments.get("subagent_type")?.as_str()?;
    let summary = active_routing_summary(messages, system)?;
    let worker = summary
        .get("selected_workers")?
        .as_array()?
        .iter()
        .find(|worker| worker.get("agent").and_then(Value::as_str) == Some(agent))?;
    Some((
        worker.get("model")?.as_str()?.to_owned(),
        worker
            .get("effort")
            .and_then(Value::as_str)
            .filter(|effort| valid_effort(effort))
            .map(str::to_owned),
    ))
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
    current_user_requests_model(messages, model)
        || selected_worker_model_matches(messages, system, model)
        || configured_advisor_model_matches(arguments, messages, system, model)
}

pub(super) fn model_is_authorized_with_catalog(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
    model: &str,
) -> bool {
    model_is_authorized(arguments, messages, system, model)
        || arguments.get(IMPLICIT_MODEL).and_then(Value::as_str) == Some(model)
        || model_catalog
            .worker_fields(
                arguments
                    .get("subagent_type")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .is_some_and(|(configured, _)| configured == model)
}

fn selected_worker_model_matches(messages: &[Value], system: &Value, model: &str) -> bool {
    active_routing_summary(messages, system).is_some_and(|summary| {
        summary
            .get("selected_workers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|worker| worker.get("model").and_then(Value::as_str) == Some(model))
    })
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

fn current_user_requests_model(messages: &[Value], model: &str) -> bool {
    // A resumed Claude Code turn may end with only "continue" after the original model choice.
    // Keep explicit user authorization across the retained conversation; the request-level
    // disabled-model policy is still enforced before any provider request is started.
    user_message_texts(messages).any(|text| {
        let explicit = text
            .split_once("{\"providers\":")
            .map_or(text, |(before_routing_context, _)| before_routing_context);
        contains_model_id(explicit, model)
    })
}

fn contains_model_id(text: &str, model: &str) -> bool {
    text.match_indices(model).any(|(start, _)| {
        let end = start + model.len();
        let before_is_boundary = text[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !is_model_id_character(character));
        before_is_boundary && model_id_ends_at_boundary(&text[end..])
    })
}

fn model_id_ends_at_boundary(remaining: &str) -> bool {
    let mut characters = remaining.chars();
    match characters.next() {
        None => true,
        Some(character) if !is_model_id_character(character) => true,
        Some(character @ ('.' | ':')) => characters
            .next()
            .is_none_or(|next| !is_model_id_character(next) || next == character),
        Some(_) => false,
    }
}

fn is_model_id_character(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || matches!(character, '-' | '_' | '.' | ':' | '/' | '@' | '+')
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
