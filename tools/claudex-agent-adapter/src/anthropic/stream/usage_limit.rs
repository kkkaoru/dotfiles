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

/// Provider-scoped quota/balance exhaustion (Qwen token plans, Grok balance,
/// and similar ACP billing windows).
/// Must not be folded into classic Codex usage-limit, which cools down the
/// whole app-server backend and would take luna/spark with it.
pub(crate) fn contains_provider_quota_exhausted_marker(value: &str) -> bool {
    let structured_payment_required = contains_structured_payment_required_status(value);
    let value = value.to_lowercase();
    const QUOTA_MARKERS: [&str; 4] = [
        "quota exhausted",
        "token-plan",
        "token plan",
        "1-week quota",
    ];
    let legacy_quota_marker = QUOTA_MARKERS
        .into_iter()
        .any(|marker| value.contains(marker))
        || contains_opencode_quota_marker(&value);
    structured_payment_required == Some(true)
        || legacy_quota_marker
        || (structured_payment_required.is_none() && contains_grok_payment_required_marker(&value))
}

/// Grok's ACP failure is wrapped in a non-JSON app-server error, with the
/// nested JSON quotes still escaped. Keep this fallback narrow: require both
/// the provider-specific balance phrase and a bounded `http_status` key whose
/// numeric value is exactly 402. This covers direct, quoted, and escaped-quoted
/// keys without restoring phrase-only cooldowns for unrelated provider prose.
fn contains_grok_payment_required_marker(value: &str) -> bool {
    value.contains("usage balance exhausted") && contains_http_status_402_token(value)
}

fn contains_http_status_402_token(value: &str) -> bool {
    value.match_indices("http_status").any(|(offset, key)| {
        let before = value[..offset].chars().next_back();
        is_status_token_boundary(before) && status_key_suffix_is_402(&value[offset + key.len()..])
    })
}

fn status_key_suffix_is_402(suffix: &str) -> bool {
    let mut suffix = suffix;
    let escaped_quote_count = suffix.bytes().take_while(|byte| *byte == b'\\').count();
    suffix = &suffix[escaped_quote_count..];
    if escaped_quote_count > 0 || suffix.starts_with('"') {
        let Some(quoted) = suffix.strip_prefix('"') else {
            return false;
        };
        suffix = quoted;
    }
    let Some(value) = suffix.strip_prefix(':') else {
        return false;
    };
    let value = value.trim_start_matches(char::is_whitespace);
    let Some(after) = value.strip_prefix("402") else {
        return false;
    };
    is_status_token_boundary(after.chars().next())
}

fn is_status_token_boundary(character: Option<char>) -> bool {
    character.is_none_or(|character| !character.is_ascii_alphanumeric() && character != '_')
}

/// Only a syntactically valid JSON value may contribute a numeric 402, and
/// only beneath an explicit HTTP-status key. Do not treat arbitrary numbers or
/// generic `status` fields as billing exhaustion. A JSON string may contain one
/// escaped JSON payload, but no unstructured substring is parsed.
fn contains_structured_payment_required_status(value: &str) -> Option<bool> {
    serde_json::from_str::<Value>(value)
        .ok()
        .map(|value| value_contains_payment_required_status(&value, true))
}

fn value_contains_payment_required_status(value: &Value, parse_escaped_json: bool) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, nested)| {
            matches!(key.as_str(), "http_status" | "httpStatusCode") && nested.as_u64() == Some(402)
                || value_contains_payment_required_status(nested, parse_escaped_json)
        }),
        Value::Array(items) => items
            .iter()
            .any(|item| value_contains_payment_required_status(item, parse_escaped_json)),
        Value::String(text) if parse_escaped_json => serde_json::from_str::<Value>(text)
            .ok()
            .is_some_and(|nested| value_contains_payment_required_status(&nested, false)),
        _ => false,
    }
}

/// OpenCode prints weekly/monthly caps on stderr and never completes prompt.
/// Must not be folded into classic Codex usage-limit (that cools the whole
/// app-server backend).
pub(crate) fn contains_opencode_quota_marker(value: &str) -> bool {
    let value = value.to_lowercase();
    const OPENCODE_QUOTA_MARKERS: [&str; 3] = [
        "weekly usage limit",
        "monthly usage limit",
        "usage limit reached",
    ];
    OPENCODE_QUOTA_MARKERS
        .into_iter()
        .any(|marker| value.contains(marker))
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
