use serde_json::Value;

use super::texts::{message_texts, routing_summary, user_message_texts, value_texts};

#[cfg(test)]
thread_local! {
    static ROUTING_SUMMARY_SEARCHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Run a test operation while deterministically counting routing-summary
/// searches on this thread. This is intentionally thread-local because the
/// adapter test suite runs in parallel.
#[cfg(test)]
pub(in crate::anthropic) fn count_routing_summary_searches<T>(
    operation: impl FnOnce() -> T,
) -> (T, usize) {
    ROUTING_SUMMARY_SEARCHES.with(|count| {
        let prior = count.replace(0);
        let result = operation();
        let searches = count.replace(prior);
        (result, searches)
    })
}

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
    #[cfg(test)]
    ROUTING_SUMMARY_SEARCHES.with(|count| count.set(count.get() + 1));
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
