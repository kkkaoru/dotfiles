use anyhow::{Context, anyhow};

use super::*;

#[test]
fn marks_missing_provider_environment_as_non_retryable() {
    let error = Err::<(), _>(anyhow!("Missing environment variable: SAKANA_AI_API_KEY"))
        .context("provider request failed")
        .expect_err("fixture error");

    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn marks_sakana_invalid_api_key_as_unauthorized() {
    let error = anyhow!(
        "codex app-server turn failed: unexpected status 401 Unauthorized: Invalid API key, url: https://api.sakana.ai/v1/responses"
    );
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::UNAUTHORIZED
    );
}

#[test]
fn preserves_retryable_provider_failures() {
    let error = anyhow!("provider connection closed");

    assert_eq!(error_type(&error), RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_GATEWAY
    );
}

#[test]
fn does_not_treat_an_unresolved_provider_phrase_as_terminal() {
    let error = anyhow!("model provider is unavailable");
    assert_eq!(error_type(&error), RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_GATEWAY
    );
}

#[test]
fn marks_blocked_subagent_launches_as_non_retryable() {
    let error = anyhow!("SubAgent model `qwen` is disabled by the active Claudex policy");

    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn marks_unknown_subagent_models_as_non_retryable() {
    let error = anyhow!("SubAgent model `` does not have a recoverable configured route");

    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn marks_invalid_main_model_routing_as_non_retryable() {
    for error in [
        anyhow!("request model is required; no provider main model is selected"),
        anyhow!("configured provider model `offline` does not have an active route"),
    ] {
        assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
        assert_eq!(
            http_status(StatusCode::BAD_GATEWAY, &error),
            StatusCode::BAD_REQUEST
        );
    }
}

#[test]
fn marks_missing_codex_model_provider_as_non_retryable() {
    let error = anyhow!("failed to load configuration: Model provider `sakana` not found");

    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn marks_missing_pi_route_extensions_as_non_retryable() {
    let error = anyhow!(
        "start Pi route provider `cursor` model `auto`: Pi route extension is missing or not a file: /missing/cursor.ts"
    );

    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn marks_context_limit_failures_as_non_retryable() {
    let error = anyhow!(
        "Claude subscription failed: Prompt is too long; the request exceeds the context limit"
    );

    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn maps_typed_subscription_failures_without_retry_storms() {
    let upstream = super::super::subscription::validate_subscription_result_for_model(
        &serde_json::json!({
            "subtype":"error", "is_error":true, "status":503,
            "result":"Service unavailable"
        }),
        Some("claude-test"),
    )
    .expect_err("upstream failure");
    assert_eq!(error_type(&upstream), RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &upstream),
        StatusCode::SERVICE_UNAVAILABLE
    );

    for (result, expected_status) in [
        ("Authentication failed", StatusCode::UNAUTHORIZED),
        ("Model not found", StatusCode::BAD_REQUEST),
        (
            "Prompt is too long for the context window",
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        let error = super::super::subscription::validate_subscription_result_for_model(
            &serde_json::json!({
                "subtype":"error", "is_error":true, "result":result
            }),
            Some("claude-test"),
        )
        .expect_err("terminal subscription failure");
        assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
        assert_eq!(
            http_status(StatusCode::BAD_GATEWAY, &error),
            expected_status
        );
    }

    let local = super::super::subscription::failure::protocol_failure(
        Some("claude-test"),
        "invalid local child output",
    );
    assert_eq!(error_type(&local), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &local),
        StatusCode::FAILED_DEPENDENCY
    );
}

#[test]
fn marks_provider_429_as_non_retryable_rate_limit() {
    let error = anyhow::Error::msg(
        r#"codex app-server turn failed: {"error":{"codexErrorInfo":{"responseTooManyFailedAttempts":{"httpStatusCode":429}},"message":"exceeded retry limit, last status: 429 Too Many Requests"}}"#,
    );
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[test]
fn marks_qwen_token_plan_quota_as_non_retryable_instead_of_502() {
    let error = anyhow!(
        "API Error: 502 codex app-server turn failed: Configured ACP prompt failed: \
Quota exhausted: Your token-plan 1-week quota has been exhausted. \
The quota will reset at 08-15 01:53:00 UTC."
    );
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[test]
fn marks_empty_acp_billing_as_non_retryable_instead_of_502() {
    let error = anyhow!(super::super::segment::EMPTY_ACP_END_TURN);
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn marks_generic_empty_assistant_turn_as_non_retryable_instead_of_502() {
    let error = anyhow!(super::super::segment::EMPTY_ASSISTANT_TURN);
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn marks_context_window_overflow_after_message_start_as_non_retryable() {
    let error = anyhow!(super::super::segment::CONTEXT_WINDOW_AFTER_MESSAGE_START);
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn marks_cline_credits_insufficient_balance_as_non_retryable_instead_of_502() {
    let error = anyhow::Error::msg(
        "codex app-server turn failed: ConfiguredLaunch ACP prompt failed: \
Error { code: -32603: Internal error, message: \"Internal error: Insufficient balance. \
Add credits at https://app.cline.bot/credits or retry with a different model.\" }",
    );
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn marks_cooling_down_provider_as_non_retryable_exhaustion() {
    let error = anyhow!("provider is cooling down after usage limit");
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn does_not_retry_provider_cooldown_502() {
    let error = anyhow!(
        "API Error: 502 grok-4.6 ACP model `grok-4.6` is cooling down after a no-event prompt timeout"
    );
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
    assert!(!crate::anthropic::token_efficiency::should_retry_provider_failure(&error));
}

#[test]
fn does_not_retry_input_validation_error() {
    let error = anyhow!("InputValidationError: prompt: Required");
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
    assert!(!crate::anthropic::token_efficiency::should_retry_provider_failure(&error));
}

#[test]
fn marks_incomplete_bash_tool_json_as_non_retryable() {
    let error =
        anyhow!("Incomplete Bash tool JSON was not flushed; a non-empty command is required.");
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn marks_empty_tool_json_circuit_as_non_retryable() {
    let error =
        anyhow!("Stopped emitting tool_use after 3 consecutive empty or invalid JSON payloads.");
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn marks_plain_401_unauthorized_phrase_as_auth_failure() {
    let error = anyhow!("upstream rejected the call: 401 Unauthorized");
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::UNAUTHORIZED
    );
}

#[test]
fn marks_classic_usage_limit_wording_as_bad_request() {
    let error = anyhow!("You've hit your usage limit. Try again later.");
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn marks_opencode_weekly_usage_limit_as_429() {
    let error = anyhow!("AI_APICallError: Weekly usage limit reached. Resets in 4 days.");
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[test]
fn marks_unexpected_status_401_marker_as_auth_failure() {
    let error = anyhow!("provider request failed: unexpected status 401 from gateway");
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::UNAUTHORIZED
    );
}

#[test]
fn marks_401_unauthorized_wording_as_auth_and_terminal() {
    let error = anyhow!("gateway rejected the key: 401 Unauthorized");
    assert_eq!(error_type(&error), NON_RETRYABLE_ERROR_TYPE);
    assert_eq!(
        http_status(StatusCode::BAD_GATEWAY, &error),
        StatusCode::UNAUTHORIZED
    );
}
