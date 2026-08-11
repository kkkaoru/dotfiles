use std::fmt;

use anyhow::{Error, Result};
use serde_json::Value;
use tokio::process::{Child, ChildStdin, Command};
use uuid::Uuid;

mod classification;
mod process;
mod results;
mod sanitize;
use classification::{classify_failure, extract_diagnostic, status_hint};
pub(in crate::anthropic) use process::process_failure;
#[cfg(test)]
use process::process_failure_from_parts;
#[allow(unused_imports)] // re-exported for crate::anthropic::subscription callers
pub(in crate::anthropic) use results::{
    after_stream_output, local_failure, protocol_failure, result_failure,
    subscription_result_for_model, subscription_result_text, timeout_failure,
    validate_subscription_result_for_model,
};
#[cfg(test)]
pub(in crate::anthropic) use results::{subscription_result, validate_subscription_result};
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
    pub(super) fn new(
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

#[cfg(test)]
mod tests;
