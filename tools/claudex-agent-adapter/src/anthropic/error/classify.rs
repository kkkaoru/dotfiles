use anyhow::Error;

use crate::anthropic::usage_limit_failover;

const MISSING_ENVIRONMENT_VARIABLE_MARKER: &str = "missing environment variable";
const INVALID_API_KEY_MARKER: &str = "invalid api key";
const UNAUTHORIZED_STATUS_MARKER: &str = "unexpected status 401";
const BLOCKED_SUBAGENT_MARKER: &str = "disabled by the active claudex policy";
const UNKNOWN_SUBAGENT_MODEL_MARKER: &str = "does not have a recoverable configured route";
const MISSING_REQUEST_MODEL_MARKER: &str = "request model is required";
const UNAVAILABLE_PROVIDER_MODEL_MARKER: &str = "does not have an active route";
const MISSING_MODEL_PROVIDER_MARKER: &str = "model provider";
const MISSING_PI_ROUTE_EXTENSION_MARKER: &str = "pi route extension is missing or not a file";
// Keep generic context-window errors on the session layer: it owns the
// one-time fresh-thread retry. Only the provider's explicit oversized-prompt
// diagnostic is terminal here, preventing Claude Code's retry storm without
// changing the existing context-recovery contract.
const OVERSIZED_PROMPT_MARKER: &str = "prompt is too long";
const INCOMPLETE_TOOL_JSON_PREFIX: &str = "incomplete ";
const INCOMPLETE_TOOL_JSON_MARKER: &str = " tool json";
const EMPTY_TOOL_JSON_CIRCUIT_MARKER: &str = "stopped emitting tool_use after";
const INPUT_VALIDATION_ERROR_MARKER: &str = "inputvalidationerror";
const COOLING_DOWN_MARKER: &str = "cooling down";

pub(super) fn is_provider_auth_error(error: &Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string().to_ascii_lowercase();
        message.contains(INVALID_API_KEY_MARKER)
            || message.contains(UNAUTHORIZED_STATUS_MARKER)
            || message.contains("401 unauthorized")
    })
}

pub(super) fn is_provider_exhaustion_error(error: &Error) -> bool {
    usage_limit_failover::should_failover_provider_error(error)
        || error.chain().any(|cause| {
            let message = cause.to_string().to_ascii_lowercase();
            message.contains(COOLING_DOWN_MARKER)
        })
}

pub(super) fn is_incomplete_tool_json_error(error: &Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string().to_ascii_lowercase();
        (message.contains(INCOMPLETE_TOOL_JSON_PREFIX)
            && message.contains(INCOMPLETE_TOOL_JSON_MARKER))
            || message.contains(EMPTY_TOOL_JSON_CIRCUIT_MARKER)
    })
}

pub(super) fn is_terminal_provider_configuration_error(error: &Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string().to_ascii_lowercase();
        message.contains(MISSING_ENVIRONMENT_VARIABLE_MARKER)
            || message.contains(INVALID_API_KEY_MARKER)
            || message.contains(UNAUTHORIZED_STATUS_MARKER)
            || message.contains("401 unauthorized")
            || message.contains(BLOCKED_SUBAGENT_MARKER)
            || message.contains(UNKNOWN_SUBAGENT_MODEL_MARKER)
            || message.contains(MISSING_REQUEST_MODEL_MARKER)
            || message.contains(UNAVAILABLE_PROVIDER_MODEL_MARKER)
            || (message.contains(MISSING_MODEL_PROVIDER_MARKER) && message.contains("not found"))
            || message.contains(MISSING_PI_ROUTE_EXTENSION_MARKER)
            || message.contains(OVERSIZED_PROMPT_MARKER)
            || message.contains(INPUT_VALIDATION_ERROR_MARKER)
            || super::super::segment::contains_cline_credits_balance_marker(&message)
    })
}
