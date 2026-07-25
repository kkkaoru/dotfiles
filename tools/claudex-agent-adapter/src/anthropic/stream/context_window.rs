use serde_json::Value;

pub(super) fn is_context_window_event(event: &Value) -> bool {
    let message = event.pointer("/params/error/message").and_then(Value::as_str);
    let details = event.pointer("/params/message").and_then(Value::as_str);
    let codex_error = event.pointer("/params/error/codexErrorInfo").and_then(Value::as_str);
    message.is_some_and(contains_context_window_marker)
        || details.is_some_and(contains_context_window_marker)
        || codex_error.is_some_and(contains_context_window_marker)
}

fn contains_context_window_marker(value: &str) -> bool {
    let value = value.to_lowercase();
    const CONTEXT_WINDOW_MARKER: &str = "contextwindowexceeded";
    value.contains("context window")
        || value.contains("ran out of room")
        || value.contains(CONTEXT_WINDOW_MARKER)
}
