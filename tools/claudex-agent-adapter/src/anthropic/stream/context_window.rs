use serde_json::Value;

pub(super) fn is_context_window_event(event: &Value) -> bool {
    let message = event.pointer("/params/error/message").and_then(Value::as_str);
    let details = event.pointer("/params/message").and_then(Value::as_str);
    let codex_error = event.pointer("/params/error/codexErrorInfo").and_then(Value::as_str);
    let error_code = event.pointer("/params/error/code").and_then(Value::as_str);
    let error_type = event.pointer("/params/error/type").and_then(Value::as_str);
    let error_name = event.pointer("/params/error/name").and_then(Value::as_str);
    let additional_details = event
        .pointer("/params/error/additionalDetails")
        .and_then(Value::as_str);
    message.is_some_and(contains_context_window_marker)
        || details.is_some_and(contains_context_window_marker)
        || codex_error.is_some_and(contains_context_window_marker)
        || error_code.is_some_and(contains_context_window_marker)
        || error_type.is_some_and(contains_context_window_marker)
        || error_name.is_some_and(contains_context_window_marker)
        || additional_details.is_some_and(contains_context_window_marker)
        || event_as_str(event)
            .as_deref()
            .is_some_and(contains_context_window_marker)
}

fn contains_context_window_marker(value: &str) -> bool {
    let value = value.to_lowercase();
    const CONTEXT_WINDOW_MARKERS: [&str; 5] = [
        "context window",
        "ran out of room",
        "contextwindowexceeded",
        "context_window_exceeded",
        "context limit",
    ];
    CONTEXT_WINDOW_MARKERS
        .into_iter()
        .any(|marker| value.contains(marker))
}

fn event_as_str(event: &Value) -> Option<String> {
    serde_json::to_string(event).ok()
}
