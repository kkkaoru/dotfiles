use std::time::Duration;

use anyhow::{Error, Result, anyhow};
use serde_json::Value;

use super::{
    SubscriptionFailure, SubscriptionFailureKind, classify_failure, extract_diagnostic, status_hint,
};

pub(in crate::anthropic) fn result_failure(model: Option<&str>, result: &Value) -> Error {
    let diagnostic = extract_diagnostic(result)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "subscription result reported an unspecified error".to_owned());
    let kind = classify_failure(Some(result), &diagnostic);
    anyhow!(SubscriptionFailure::new(
        kind,
        model,
        None,
        &diagnostic,
        status_hint(kind, Some(result), Some(&diagnostic)),
    ))
}

pub(in crate::anthropic) fn local_failure(model: &str, operation: &str, error: &Error) -> Error {
    anyhow!(SubscriptionFailure::new(
        SubscriptionFailureKind::LocalProcess,
        Some(model),
        None,
        format!("{operation}: {error:#}"),
        424,
    ))
}

pub(in crate::anthropic) fn timeout_failure(model: &str, timeout: Duration) -> Error {
    anyhow!(SubscriptionFailure::new(
        SubscriptionFailureKind::LocalTimeout,
        Some(model),
        None,
        format!("child process timed out after {timeout:?}"),
        424,
    ))
}

pub(in crate::anthropic) fn after_stream_output(model: &str, error: Error) -> Error {
    match error.downcast::<SubscriptionFailure>() {
        Ok(mut failure) => {
            failure.stream_output_emitted = true;
            anyhow!(failure)
        }
        Err(error) => {
            let mut failure = SubscriptionFailure::new(
                SubscriptionFailureKind::LocalProcess,
                Some(model),
                None,
                format!("stream failed after emitting frames: {error:#}"),
                424,
            );
            failure.stream_output_emitted = true;
            anyhow!(failure)
        }
    }
}

pub(in crate::anthropic) fn protocol_failure(model: Option<&str>, diagnostic: &str) -> Error {
    anyhow!(SubscriptionFailure::new(
        SubscriptionFailureKind::Protocol,
        model,
        None,
        diagnostic,
        424,
    ))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(in crate::anthropic) fn subscription_result(stdout: &[u8]) -> Result<String> {
    subscription_result_for_model(stdout, None)
}

pub(in crate::anthropic) fn subscription_result_for_model(stdout: &[u8], model: Option<&str>) -> Result<String> {
    let value: Value = serde_json::from_slice(stdout)
        .map_err(|_| protocol_failure(model, "returned invalid JSON"))?;
    validate_subscription_result_for_model(&value, model)?;
    subscription_result_text(&value)
        .ok_or_else(|| protocol_failure(model, "JSON did not contain a result"))
}

pub(in crate::anthropic) fn subscription_result_text(result: &Value) -> Option<String> {
    match result.get("structured_output") {
        Some(Value::String(text)) => Some(text.clone()),
        Some(value) if !value.is_null() => Some(value.to_string()),
        _ => result
            .get("result")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(in crate::anthropic) fn validate_subscription_result(result: &Value) -> Result<()> {
    validate_subscription_result_for_model(result, None)
}

pub(in crate::anthropic) fn validate_subscription_result_for_model(
    result: &Value,
    model: Option<&str>,
) -> Result<()> {
    if result.get("is_error").and_then(Value::as_bool) == Some(true)
        || result.get("subtype").and_then(Value::as_str) != Some("success")
    {
        return Err(result_failure(model, result));
    }
    Ok(())
}
