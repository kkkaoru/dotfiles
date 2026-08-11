use std::process::ExitStatus;

use anyhow::{Error, anyhow};
use serde_json::Value;

use super::{
    SubscriptionFailure, SubscriptionFailureKind, classify_failure, extract_diagnostic, status_hint,
};

pub(in crate::anthropic) fn process_failure(
    model: &str,
    status: &ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> Error {
    process_failure_from_parts(model, status.to_string(), stdout, stderr)
}

pub(super) fn process_failure_from_parts(
    model: &str,
    status: String,
    stdout: &[u8],
    stderr: &[u8],
) -> Error {
    let structured = serde_json::from_slice::<Value>(stdout).ok();
    let stdout_diagnostic = structured.as_ref().and_then(|value| {
        let explicitly_successful = value.get("subtype").and_then(Value::as_str) == Some("success")
            && value.get("is_error").and_then(Value::as_bool) != Some(true);
        (!explicitly_successful)
            .then(|| extract_diagnostic(value))
            .flatten()
    });
    let stderr_diagnostic = String::from_utf8_lossy(stderr).trim().to_owned();
    let diagnostic = stdout_diagnostic
        .filter(|value| !value.trim().is_empty())
        .or_else(|| (!stderr_diagnostic.is_empty()).then_some(stderr_diagnostic));
    let kind = match (&structured, diagnostic.as_deref()) {
        (_, None) => SubscriptionFailureKind::EmptyProcessOutput,
        (Some(value), Some(message)) => classify_failure(Some(value), message),
        (None, Some(message)) => match classify_failure(None, message) {
            SubscriptionFailureKind::Protocol => SubscriptionFailureKind::LocalProcess,
            classified => classified,
        },
    };
    let status_hint = status_hint(kind, structured.as_ref(), diagnostic.as_deref());
    anyhow!(SubscriptionFailure::new(
        kind,
        Some(model),
        Some(status),
        diagnostic.unwrap_or_else(|| "no diagnostic output".to_owned()),
        status_hint,
    ))
}
