use serde_json::Value;

pub(crate) fn is_auth_failure_event(event: &Value) -> bool {
    let message = event
        .pointer("/params/error/message")
        .and_then(Value::as_str);
    let details = event.pointer("/params/message").and_then(Value::as_str);
    message.is_some_and(contains_auth_failure_marker)
        || details.is_some_and(contains_auth_failure_marker)
        || event_as_str(event)
            .as_deref()
            .is_some_and(contains_auth_failure_marker)
}

pub(crate) fn contains_auth_failure_marker(value: &str) -> bool {
    let value = value.to_lowercase();
    const AUTH_FAILURE_MARKERS: [&str; 5] = [
        "invalid api key",
        "missing environment variable",
        "unexpected status 401",
        "401 unauthorized",
        "unauthorized: invalid",
    ];
    AUTH_FAILURE_MARKERS
        .into_iter()
        .any(|marker| value.contains(marker))
}

pub(crate) fn auth_scope_from_message(message: &str) -> Option<String> {
    let lower = message.to_ascii_lowercase();
    if lower.contains("sakana") || lower.contains("sakana_ai") {
        return Some("sakana".to_owned());
    }
    None
}

fn event_as_str(event: &Value) -> Option<String> {
    serde_json::to_string(event).ok()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::{auth_scope_from_message, contains_auth_failure_marker, is_auth_failure_event};

    #[test]
    fn detects_sakana_invalid_api_key_events() {
        let event = json!({
            "params":{
                "willRetry":false,
                "error":{
                    "codexErrorInfo":"other",
                    "message":"unexpected status 401 Unauthorized: Invalid API key, url: https://api.sakana.ai/v1/responses"
                }
            }
        });
        assert!(is_auth_failure_event(&event));
        assert!(contains_auth_failure_marker(
            "unexpected status 401 Unauthorized: Invalid API key, url: https://api.sakana.ai/v1/responses"
        ));
        assert_eq!(
            auth_scope_from_message(
                "unexpected status 401 Unauthorized: Invalid API key, url: https://api.sakana.ai/v1/responses"
            )
            .as_deref(),
            Some("sakana")
        );
        assert!(!contains_auth_failure_marker("usage limit exceeded"));
        assert!(!is_auth_failure_event(
            &json!({"params":{"error":{"message":"other"}}})
        ));
    }

    #[test]
    fn detects_missing_sakana_environment_variable() {
        assert!(contains_auth_failure_marker(
            "Missing environment variable: `SAKANA_AI_PRO_API_KEY`."
        ));
        assert_eq!(
            auth_scope_from_message("Missing environment variable: `SAKANA_AI_PRO_API_KEY`.")
                .as_deref(),
            Some("sakana")
        );
    }
}
