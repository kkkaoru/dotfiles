use serde_json::Value;

pub(super) fn explicit_model_matches_agent(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
    model: &str,
) -> bool {
    let Some(agent) = arguments.get("subagent_type").and_then(Value::as_str) else {
        return false;
    };
    if super::is_generic_agent_type(Some(agent)) || !active_user_requests_model(messages, model) {
        return false;
    }
    super::active_routing_summary(messages, system)
        .and_then(|summary| summary.get("providers").cloned())
        .and_then(|providers| providers.as_object().cloned())
        .into_iter()
        .flatten()
        .any(|(_, provider)| provider_accepts_model(&provider, agent, model))
}

fn provider_accepts_model(provider: &Value, agent: &str, model: &str) -> bool {
    provider.get("agent").and_then(Value::as_str) == Some(agent)
        && (provider.get("model").and_then(Value::as_str) == Some(model)
            || provider
                .get("model_prefixes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|prefix| model.starts_with(prefix)))
}

pub(super) fn active_user_requests_model(messages: &[Value], model: &str) -> bool {
    super::user_message_texts(messages)
        .filter(|text| {
            !text.contains("<agent-message")
                && !text.contains("<teammate-message")
                && !text.contains("<task-notification")
                && !text.starts_with("Another Claude session sent a message")
        })
        .last()
        .is_some_and(|text| {
            let explicit = text
                .split_once("{\"providers\":")
                .map_or(text, |(before_routing_context, _)| before_routing_context);
            contains_model_id(explicit, model)
        })
}

fn contains_model_id(text: &str, model: &str) -> bool {
    text.match_indices(model).any(|(start, _)| {
        let end = start + model.len();
        text[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !is_model_id_character(character))
            && model_id_ends_at_boundary(&text[end..])
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
