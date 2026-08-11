use anyhow::Error;
use axum::http::StatusCode;

pub(super) const RETRYABLE_ERROR_TYPE: &str = "api_error";
pub(super) const NON_RETRYABLE_ERROR_TYPE: &str = "invalid_request_error";

mod classify;
use classify::{
    is_provider_auth_error, is_provider_exhaustion_error, is_terminal_provider_configuration_error,
};

pub(super) fn error_type(error: &Error) -> &'static str {
    if let Some(failure) = super::subscription::subscription_failure(error) {
        return if failure.is_outer_retryable() {
            RETRYABLE_ERROR_TYPE
        } else {
            NON_RETRYABLE_ERROR_TYPE
        };
    }
    if is_terminal_provider_configuration_error(error) || is_provider_exhaustion_error(error) {
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
    if is_provider_auth_error(error) {
        StatusCode::UNAUTHORIZED
    } else if super::stream::usage_limit::contains_rate_limit_marker(&error.to_string())
        || super::stream::usage_limit::contains_provider_quota_exhausted_marker(&error.to_string())
    {
        StatusCode::TOO_MANY_REQUESTS
    } else if is_terminal_provider_configuration_error(error)
        || super::stream::usage_limit::contains_classic_usage_limit_marker(&error.to_string())
        || super::segment::contains_empty_acp_billing_marker(&error.to_string())
    {
        StatusCode::BAD_REQUEST
    } else {
        fallback
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
