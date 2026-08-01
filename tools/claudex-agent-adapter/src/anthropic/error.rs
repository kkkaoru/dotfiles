use anyhow::Error;
use axum::http::StatusCode;

pub(super) const RETRYABLE_ERROR_TYPE: &str = "api_error";
pub(super) const NON_RETRYABLE_ERROR_TYPE: &str = "invalid_request_error";

const MISSING_ENVIRONMENT_VARIABLE_MARKER: &str = "missing environment variable";
const BLOCKED_SUBAGENT_MARKER: &str = "disabled by the active claudex policy";
const UNKNOWN_SUBAGENT_MODEL_MARKER: &str = "does not have a recoverable configured route";
const MISSING_REQUEST_MODEL_MARKER: &str = "request model is required";
const UNAVAILABLE_PROVIDER_MODEL_MARKER: &str = "does not have an active route";
const MISSING_MODEL_PROVIDER_MARKER: &str = "model provider";
// Keep generic context-window errors on the session layer: it owns the
// one-time fresh-thread retry. Only the provider's explicit oversized-prompt
// diagnostic is terminal here, preventing Claude Code's retry storm without
// changing the existing context-recovery contract.
const OVERSIZED_PROMPT_MARKER: &str = "prompt is too long";

pub(super) fn error_type(error: &Error) -> &'static str {
    if let Some(failure) = super::subscription::subscription_failure(error) {
        return if failure.is_outer_retryable() {
            RETRYABLE_ERROR_TYPE
        } else {
            NON_RETRYABLE_ERROR_TYPE
        };
    }
    if is_terminal_provider_configuration_error(error) {
        NON_RETRYABLE_ERROR_TYPE
    } else {
        RETRYABLE_ERROR_TYPE
    }
}

pub(super) fn http_status(fallback: StatusCode, error: &Error) -> StatusCode {
    if let Some(failure) = super::subscription::subscription_failure(error) {
        return StatusCode::from_u16(failure.status_hint())
            .unwrap_or(StatusCode::FAILED_DEPENDENCY);
    }
    if is_terminal_provider_configuration_error(error) {
        StatusCode::BAD_REQUEST
    } else {
        fallback
    }
}

fn is_terminal_provider_configuration_error(error: &Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string().to_ascii_lowercase();
        message.contains(MISSING_ENVIRONMENT_VARIABLE_MARKER)
            || message.contains(BLOCKED_SUBAGENT_MARKER)
            || message.contains(UNKNOWN_SUBAGENT_MODEL_MARKER)
            || message.contains(MISSING_REQUEST_MODEL_MARKER)
            || message.contains(UNAVAILABLE_PROVIDER_MODEL_MARKER)
            || (message.contains(MISSING_MODEL_PROVIDER_MARKER) && message.contains("not found"))
            || message.contains(OVERSIZED_PROMPT_MARKER)
    })
}

#[cfg(test)]
mod tests {
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
}
