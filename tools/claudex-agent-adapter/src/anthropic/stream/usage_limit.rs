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
    contains_classic_usage_limit_marker(value)
        || contains_rate_limit_marker(value)
        || contains_provider_quota_exhausted_marker(value)
}

/// Qwen Cloud token-plan / similar ACP billing windows.
/// Must not be folded into classic Codex usage-limit, which cools down the
/// whole app-server backend and would take luna/spark with it.
pub(crate) fn contains_provider_quota_exhausted_marker(value: &str) -> bool {
    let value = value.to_lowercase();
    const QUOTA_MARKERS: [&str; 4] = [
        "quota exhausted",
        "token-plan",
        "token plan",
        "1-week quota",
    ];
    QUOTA_MARKERS
        .into_iter()
        .any(|marker| value.contains(marker))
}

pub(crate) fn contains_classic_usage_limit_marker(value: &str) -> bool {
    let value = value.to_lowercase();
    // Include OpenCode Go weekly-cap wording ("Weekly usage limit reached…") so
    // configured-ACP failures cool down routing instead of silent 502/retry storms.
    const USAGE_LIMIT_MARKERS: [&str; 7] = [
        "usagelimitexceeded",
        "usage_limit_exceeded",
        "usage limit exceeded",
        "usage limit reached",
        "weekly usage limit",
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
        Value::Object(map) => map.iter().any(|(key, nested)| {
            contains_usage_limit_marker(key) || value_contains_usage_limit(nested)
        }),
        Value::Array(items) => items.iter().any(value_contains_usage_limit),
        _ => false,
    }
}

fn event_as_str(event: &Value) -> Option<String> {
    serde_json::to_string(event).ok()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "usage_limit_tests.rs"]
mod tests;
