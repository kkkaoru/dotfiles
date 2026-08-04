use serde_json::Value;

pub(super) fn is_usage_limit_event(event: &Value) -> bool {
    let message = event
        .pointer("/params/error/message")
        .and_then(Value::as_str);
    let details = event.pointer("/params/message").and_then(Value::as_str);
    let codex_error = event
        .pointer("/params/error/codexErrorInfo")
        .and_then(Value::as_str);
    let error_code = event.pointer("/params/error/code").and_then(Value::as_str);
    let error_type = event.pointer("/params/error/type").and_then(Value::as_str);
    let error_name = event.pointer("/params/error/name").and_then(Value::as_str);
    let additional_details = event
        .pointer("/params/error/additionalDetails")
        .and_then(Value::as_str);
    message.is_some_and(contains_usage_limit_marker)
        || details.is_some_and(contains_usage_limit_marker)
        || codex_error.is_some_and(contains_usage_limit_marker)
        || error_code.is_some_and(contains_usage_limit_marker)
        || error_type.is_some_and(contains_usage_limit_marker)
        || error_name.is_some_and(contains_usage_limit_marker)
        || additional_details.is_some_and(contains_usage_limit_marker)
        || event_as_str(event)
            .as_deref()
            .is_some_and(contains_usage_limit_marker)
}

pub(crate) fn contains_usage_limit_marker(value: &str) -> bool {
    let value = value.to_lowercase();
    const USAGE_LIMIT_MARKERS: [&str; 5] = [
        "usagelimitexceeded",
        "usage_limit_exceeded",
        "usage limit exceeded",
        "hit your usage limit",
        "you've hit your usage limit",
    ];
    USAGE_LIMIT_MARKERS
        .into_iter()
        .any(|marker| value.contains(marker))
}

fn event_as_str(event: &Value) -> Option<String> {
    serde_json::to_string(event).ok()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::{contains_usage_limit_marker, is_usage_limit_event};

    #[test]
    fn detects_codex_usage_limit_events() {
        let event = json!({
            "params":{
                "willRetry":false,
                "error":{
                    "codexErrorInfo":"usageLimitExceeded",
                    "message":"You've hit your usage limit. Try again at 3:20 AM."
                }
            }
        });
        assert!(is_usage_limit_event(&event));
        assert!(contains_usage_limit_marker(
            "You've hit your usage limit for GPT-5.3-Codex-Spark."
        ));
        assert!(!contains_usage_limit_marker("context window exceeded"));
        assert!(!is_usage_limit_event(&json!({"params":{"error":{"message":"other"}}})));
    }
}
