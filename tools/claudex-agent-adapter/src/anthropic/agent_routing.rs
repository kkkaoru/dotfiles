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
    arguments: &Value,
    messages: &[Value],
    system: &Value,
    model: &str,
) -> bool {
    current_user_requests_model(messages, model)
        || selected_worker_matches(arguments, messages, system, model)
}

fn selected_worker_matches(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
    model: &str,
) -> bool {
    let Some(agent) = arguments
        .get("subagent_type")
        .or_else(|| arguments.get("agent"))
        .and_then(Value::as_str)
    else {
        return false;
    };
    user_message_texts(messages)
        .chain(value_texts(system))
        .filter_map(routing_summary)
        .last()
        .is_some_and(|summary| {
            summary
                .get("selected_workers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|worker| {
                    worker.get("agent").and_then(Value::as_str) == Some(agent)
                        && worker.get("model").and_then(Value::as_str) == Some(model)
                })
        })
}

fn current_user_requests_model(messages: &[Value], model: &str) -> bool {
    messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|message| message.get("content"))
        .into_iter()
        .flat_map(value_texts)
        .any(|text| {
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

fn value_texts(value: &Value) -> impl Iterator<Item = &str> {
    value.as_str().into_iter().chain(
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| block.get("text").and_then(Value::as_str)),
    )
}
