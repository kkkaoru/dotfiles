use serde_json::Value;

pub(super) fn is_usage_limit_event(event: &Value) -> bool {
    let message = event
        .pointer("/params/error/message")
        .and_then(Value::as_str);
    let details = event.pointer("/params/message").and_then(Value::as_str);
    let error_code = event.pointer("/params/error/code").and_then(Value::as_str);
    let error_type = event.pointer("/params/error/type").and_then(Value::as_str);
    let error_name = event.pointer("/params/error/name").and_then(Value::as_str);
    let additional_details = event
        .pointer("/params/error/additionalDetails")
        .and_then(Value::as_str);
    message.is_some_and(contains_usage_limit_marker)
        || details.is_some_and(contains_usage_limit_marker)
        || error_code.is_some_and(contains_usage_limit_marker)
        || error_type.is_some_and(contains_usage_limit_marker)
        || error_name.is_some_and(contains_usage_limit_marker)
        || additional_details.is_some_and(contains_usage_limit_marker)
        || event
            .pointer("/params/error/codexErrorInfo")
            .is_some_and(value_contains_usage_limit)
        || event
            .pointer("/params/error")
            .is_some_and(value_contains_usage_limit)
        || event_as_str(event)
            .as_deref()
            .is_some_and(contains_usage_limit_marker)
}

pub(crate) fn contains_usage_limit_marker(value: &str) -> bool {
    contains_classic_usage_limit_marker(value) || contains_rate_limit_marker(value)
}

pub(crate) fn contains_classic_usage_limit_marker(value: &str) -> bool {
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

pub(crate) fn contains_rate_limit_marker(value: &str) -> bool {
    let value = value.to_lowercase();
    const RATE_LIMIT_MARKERS: [&str; 6] = [
        "too many requests",
        "responsetoomanyfailedattempts",
        "last status: 429",
        "httpstatuscode\":429",
        "\"status\":429",
        "rate limit",
    ];
    RATE_LIMIT_MARKERS
        .into_iter()
        .any(|marker| value.contains(marker))
}

fn value_contains_usage_limit(value: &Value) -> bool {
    match value {
        Value::String(text) => contains_usage_limit_marker(text),
        Value::Number(number) => number.as_u64() == Some(429),
        Value::Object(map) => map
            .iter()
            .any(|(key, nested)| contains_usage_limit_marker(key) || value_contains_usage_limit(nested)),
        Value::Array(items) => items.iter().any(value_contains_usage_limit),
        _ => false,
    }
}

fn event_as_str(event: &Value) -> Option<String> {
    serde_json::to_string(event).ok()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::{
        contains_classic_usage_limit_marker, contains_rate_limit_marker, contains_usage_limit_marker,
        is_usage_limit_event,
    };

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
        assert!(!is_usage_limit_event(
            &json!({"params":{"error":{"message":"other"}}})
        ));
    }

    #[test]
    fn detects_object_shaped_429_codex_errors() {
        let event = json!({
            "params":{
                "willRetry":false,
                "error":{
                    "additionalDetails":null,
                    "codexErrorInfo":{
                        "responseTooManyFailedAttempts":{"httpStatusCode":429}
                    },
                    "message":"exceeded retry limit, last status: 429 Too Many Requests"
                }
            }
        });
        assert!(is_usage_limit_event(&event));
        assert!(contains_rate_limit_marker(
            "exceeded retry limit, last status: 429 Too Many Requests, request id: abc"
        ));
        assert!(contains_usage_limit_marker(
            "codex app-server turn failed: responseTooManyFailedAttempts httpStatusCode\":429"
        ));
        assert!(!contains_classic_usage_limit_marker(
            "exceeded retry limit, last status: 429 Too Many Requests"
        ));
    }
}
