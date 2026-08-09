use std::{fmt, process::ExitStatus, time::Duration};

use anyhow::{Error, Result, anyhow};
use serde_json::Value;
use tokio::process::{Child, ChildStdin, Command};
use uuid::Uuid;

mod classification;
mod sanitize;
use classification::{classify_failure, extract_diagnostic, status_hint};
use sanitize::sanitize_diagnostic;

const MAX_DIAGNOSTIC_CHARS: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SubscriptionFailureKind {
    UpstreamTransient,
    Authentication,
    Configuration,
    ContextLimit,
    LocalProcess,
    LocalTimeout,
    EmptyProcessOutput,
    Protocol,
}

impl SubscriptionFailureKind {
    fn label(self) -> &'static str {
        match self {
            Self::UpstreamTransient => "upstream_transient",
            Self::Authentication => "authentication",
            Self::Configuration => "configuration",
            Self::ContextLimit => "context_limit",
            Self::LocalProcess => "local_process",
            Self::LocalTimeout => "local_timeout",
            Self::EmptyProcessOutput => "empty_process_output",
            Self::Protocol => "protocol",
        }
    }
}

#[derive(Debug)]
pub(in crate::anthropic) struct SubscriptionFailure {
    kind: SubscriptionFailureKind,
    diagnostic: String,
    model: Option<String>,
    process_status: Option<String>,
    status_hint: u16,
    trace_id: String,
    stream_output_emitted: bool,
}

impl SubscriptionFailure {
    fn new(
        kind: SubscriptionFailureKind,
        model: Option<&str>,
        process_status: Option<String>,
        diagnostic: impl AsRef<str>,
        status_hint: u16,
    ) -> Self {
        Self {
            kind,
            diagnostic: sanitize_diagnostic(diagnostic.as_ref()),
            model: model.map(str::to_owned),
            process_status,
            status_hint,
            trace_id: Uuid::new_v4().simple().to_string(),
            stream_output_emitted: false,
        }
    }

    pub(in crate::anthropic) fn is_internal_retryable(&self) -> bool {
        !self.stream_output_emitted
            && matches!(
                self.kind,
                SubscriptionFailureKind::LocalProcess | SubscriptionFailureKind::EmptyProcessOutput
            )
    }

    pub(in crate::anthropic) fn is_outer_retryable(&self) -> bool {
        self.kind == SubscriptionFailureKind::UpstreamTransient
    }

    pub(in crate::anthropic) fn is_authentication(&self) -> bool {
        self.kind == SubscriptionFailureKind::Authentication
    }

    pub(in crate::anthropic) fn status_hint(&self) -> u16 {
        self.status_hint
    }
}

impl fmt::Display for SubscriptionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Claude subscription")?;
        if let Some(model) = &self.model {
            write!(formatter, " model {model}")?;
        }
        write!(formatter, " failed [{}", self.kind.label())?;
        if let Some(status) = &self.process_status {
            write!(formatter, "; {status}")?;
        }
        if self.stream_output_emitted {
            formatter.write_str("; subscription stream already emitted frames")?;
        }
        write!(formatter, "; trace={}]: {}", self.trace_id, self.diagnostic)
    }
}

impl std::error::Error for SubscriptionFailure {}

pub(in crate::anthropic) fn subscription_failure(error: &Error) -> Option<&SubscriptionFailure> {
    error.downcast_ref::<SubscriptionFailure>()
}

pub(in crate::anthropic) fn spawn_child(
    command: &mut Command,
    model: &str,
) -> Result<(Child, ChildStdin)> {
    let mut child = super::spawn_subscription(command, model)
        .map_err(|error| local_failure(model, "failed to start child process", &error))?;
    let stdin = super::take_subscription_stdin(&mut child)
        .map_err(|error| local_failure(model, "failed to access child stdin", &error))?;
    Ok((child, stdin))
}

pub(in crate::anthropic) fn local_result<T>(
    model: &str,
    operation: &str,
    result: Result<T>,
) -> Result<T> {
    result.map_err(|error| local_failure(model, operation, &error))
}

pub(in crate::anthropic) fn parse_stream_envelope(
    model: Option<&str>,
    line: &str,
) -> Result<Value> {
    serde_json::from_str(line).map_err(|_| protocol_failure(model, "emitted invalid stream JSON"))
}

pub(in crate::anthropic) fn process_failure(
    model: &str,
    status: &ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> Error {
    process_failure_from_parts(model, status.to_string(), stdout, stderr)
}

fn process_failure_from_parts(model: &str, status: String, stdout: &[u8], stderr: &[u8]) -> Error {
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

pub(super) fn result_failure(model: Option<&str>, result: &Value) -> Error {
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

pub(super) fn local_failure(model: &str, operation: &str, error: &Error) -> Error {
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

pub(super) fn subscription_result_for_model(stdout: &[u8], model: Option<&str>) -> Result<String> {
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

#[cfg(test)]
mod tests;
