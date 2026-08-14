use serde_json::json;

use super::{
    contains_classic_usage_limit_marker, contains_provider_quota_exhausted_marker,
    contains_rate_limit_marker, contains_usage_limit_marker, is_usage_limit_event,
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
    assert!(contains_usage_limit_marker(
        "AI_APICallError: Weekly usage limit reached. Resets in 4 days."
    ));
    assert!(!contains_usage_limit_marker("context window exceeded"));
    assert!(!is_usage_limit_event(
        &json!({"params":{"error":{"message":"other"}}})
    ));
}

#[test]
fn detects_opencode_weekly_quota_without_classic_codex_usage_limit() {
    const OPENCODE_WEEKLY: &str = "AI_APICallError: Weekly usage limit reached. Resets in 4 days.";
    assert!(contains_provider_quota_exhausted_marker(OPENCODE_WEEKLY));
    assert!(contains_usage_limit_marker(OPENCODE_WEEKLY));
    assert!(
        !contains_classic_usage_limit_marker(OPENCODE_WEEKLY),
        "OpenCode weekly cap must not cool down the Codex app-server backend"
    );
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

#[test]
fn detects_qwen_token_plan_quota_without_classic_codex_usage_limit() {
    // Exact TUI / ACP wording from fa522331 multi-SubAgent launch
    // (`claudex_model: qwen3.8-max-preview`).
    const TUI_QWEN_QUOTA: &str = "API Error: 502 codex app-server turn failed: \
Configured ACP prompt failed: Quota exhausted: Your token-plan 1-week quota has been exhausted. \
The quota will reset at 08-15 01:53:00 UTC.";
    assert!(contains_provider_quota_exhausted_marker(TUI_QWEN_QUOTA));
    assert!(contains_usage_limit_marker(TUI_QWEN_QUOTA));
    assert!(
        !contains_classic_usage_limit_marker(TUI_QWEN_QUOTA),
        "Qwen token-plan must not cool down the Codex app-server backend"
    );
    assert!(is_usage_limit_event(&json!({
        "params": {
            "willRetry": false,
            "error": {
                "message": "Configured ACP prompt failed",
                "data": {
                    "details": "Quota exhausted: Your token-plan 1-week quota has been exhausted."
                }
            }
        }
    })));
    assert!(!contains_provider_quota_exhausted_marker(
        "context window exceeded"
    ));
}

#[test]
fn detects_grok_payment_required_balance_exhaustion_in_text_and_json() {
    // Observed from Grok ACP: a prepaid balance failure is reported as a
    // provider-scoped 402 rather than a Codex app-server-wide usage limit.
    const GROK_402_BALANCE_EXHAUSTED: &str = "Configured ACP prompt failed: \
http_status:402 Payment Required: usage balance exhausted";

    assert!(contains_provider_quota_exhausted_marker(
        GROK_402_BALANCE_EXHAUSTED
    ));
    assert!(contains_usage_limit_marker(GROK_402_BALANCE_EXHAUSTED));
    assert!(contains_provider_quota_exhausted_marker(
        "USAGE BALANCE EXHAUSTED"
    ));
    for status_json in [
        r#"{"http_status":402,"message":"Payment Required"}"#,
        r#"{"message":"Payment Required","http_status": 402}"#,
        "{\n  \"message\": \"Payment Required\",\n  \"http_status\": 402\n}",
        r#"{"outer":{"httpStatusCode":402}}"#,
        r#"{"outer":{"provider":{"http_status":402}}}"#,
    ] {
        assert!(
            contains_provider_quota_exhausted_marker(status_json),
            "allowlisted numeric HTTP 402 must be recognized: {status_json}"
        );
    }
    let escaped_status_json =
        serde_json::to_string(r#"{"error":{"httpStatusCode":402}}"#).expect("escaped JSON fixture");
    assert!(contains_provider_quota_exhausted_marker(
        &escaped_status_json
    ));
    assert!(is_usage_limit_event(&json!({
        "params": {
            "willRetry": false,
            "error": {
                "message": GROK_402_BALANCE_EXHAUSTED,
                "data": {"provider": {"http_status": 402}}
            }
        }
    })));
    assert!(contains_provider_quota_exhausted_marker(
        "Quota exhausted: Your token-plan 1-week quota has been exhausted."
    ));
    assert!(contains_provider_quota_exhausted_marker(
        "AI_APICallError: Weekly usage limit reached. Resets in 4 days."
    ));
    assert!(
        !contains_classic_usage_limit_marker(GROK_402_BALANCE_EXHAUSTED),
        "Grok prepaid balance must not cool down the Codex app-server backend"
    );
}

#[test]
fn grok_payment_required_rejects_other_statuses_and_ordinary_balances() {
    for near_miss in [
        "http_status:402",
        "grok upstream emitted http_status:402",
        "http_status:403 Forbidden",
        "http_status:4020 malformed status",
        "Current usage balance: $12.34",
        r#"{"http_status":"402","message":"Payment Required"}"#,
        r#"{"status":402,"message":"Payment Required"}"#,
        r#"{"http_status":403,"message":"Payment Required"}"#,
    ] {
        assert!(
            !contains_provider_quota_exhausted_marker(near_miss),
            "near miss must not trigger a provider cooldown: {near_miss}"
        );
        assert!(
            !contains_usage_limit_marker(near_miss),
            "near miss must not trigger usage-limit failover: {near_miss}"
        );
    }
}

#[test]
fn detects_usage_limit_markers_on_each_error_field() {
    assert!(is_usage_limit_event(&json!({
        "params": {"message": "You've hit your usage limit"}
    })));
    assert!(is_usage_limit_event(&json!({
        "params": {"error": {"code": "usage_limit_exceeded"}}
    })));
    assert!(is_usage_limit_event(&json!({
        "params": {"error": {"type": "rate limit"}}
    })));
    assert!(is_usage_limit_event(&json!({
        "params": {"error": {"name": "UsageLimitExceeded"}}
    })));
    assert!(is_usage_limit_event(&json!({
        "params": {"error": {"additionalDetails": "quota exhausted token-plan"}}
    })));
    assert!(is_usage_limit_event(
        &json!({"error": "weekly usage limit reached"})
    ));
    assert!(!is_usage_limit_event(
        &json!({"params": {"error": {"code": "other"}}})
    ));
}

#[test]
fn detects_numeric_429_inside_codex_error_info() {
    assert!(is_usage_limit_event(&json!({
        "params":{"error":{"codexErrorInfo":429}}
    })));
    assert!(!is_usage_limit_event(&json!({
        "params":{"error":{"codexErrorInfo":200}}
    })));
}
