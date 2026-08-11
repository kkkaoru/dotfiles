use serde_json::Value;

use super::texts::{message_texts, routing_summary, user_message_texts, value_texts};

pub(super) fn advisor_launch_disabled(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
) -> bool {
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

pub(super) fn configured_advisor_model_matches(
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

pub(in crate::anthropic) fn active_routing_summary(
    messages: &[Value],
    system: &Value,
) -> Option<Value> {
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
