use serde::Deserialize;
use serde_json::Value;

use super::subscription::valid_effort;

const ADAPTER_EFFORT: &str = "claudex_effort";
const ADAPTER_MODEL: &str = "claudex_model";

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
    _arguments: &Value,
    messages: &[Value],
    system: &Value,
    model: &str,
) -> bool {
    // Claude Code can preserve its built-in `general-purpose` / `Explore` type while passing a
    // routed provider model. The selected model remains the authority; requiring its display
    // type to equal the configured worker name rejects that valid launch before ACP can start.
    current_user_requests_model(messages, model)
        || selected_worker_model_matches(messages, system, model)
}

fn selected_worker_model_matches(messages: &[Value], system: &Value, model: &str) -> bool {
    // The hook normally places the current snapshot in a user message, but Claude Code can
    // retain it in an assistant/tool transcript after compaction or a resumed turn. Prefer the
    // request-level system snapshot, then the latest user snapshot, and finally any transcript
    // snapshot so an otherwise valid routed worker is not rejected after context reshaping.
    let summary = value_texts(system)
        .filter_map(routing_summary)
        .last()
        .or_else(|| {
            user_message_texts(messages)
                .filter_map(routing_summary)
                .last()
        })
        .or_else(|| message_texts(messages).filter_map(routing_summary).last());
    summary.is_some_and(|summary| {
        summary
            .get("selected_workers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|worker| worker.get("model").and_then(Value::as_str) == Some(model))
    })
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
