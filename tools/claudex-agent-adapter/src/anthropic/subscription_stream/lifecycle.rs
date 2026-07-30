use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::{io::AsyncReadExt, process::Child};

use super::super::subscription::terminate_subscription;

pub(super) fn is_api_retry_failure(envelope: &Value) -> bool {
    envelope.get("type").and_then(Value::as_str) == Some("api_retry")
        || envelope.get("subtype").and_then(Value::as_str) == Some("api_retry")
        || envelope.pointer("/error/type").and_then(Value::as_str) == Some("authentication_failed")
}

pub(super) fn bail_api_retry(envelope: &Value) -> Result<()> {
    let error_type = envelope
        .pointer("/error/type")
        .and_then(Value::as_str)
        .or_else(|| envelope.get("error").and_then(Value::as_str))
        .unwrap_or("unknown_error");
    let message = envelope
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("provider request could not be retried");
    bail!("Claude subscription api_retry ({error_type}): {message}")
}

pub(super) async fn terminate_after_stream_failure<T>(
    child: &mut Child,
    error: anyhow::Error,
) -> Result<T> {
    terminate_subscription(child)
        .await
        .context("also failed to terminate the Claude subscription stream")?;
    Err(error)
}

pub(super) async fn terminate_closed_stream(
    child: &mut Child,
    stderr_task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
) -> Result<()> {
    let termination = terminate_subscription(child).await;
    let stderr = stderr_task.await.context("Claude stderr task failed")?;
    termination?;
    stderr?;
    Ok(())
}

pub(super) async fn terminate_failed_stream(
    child: &mut Child,
    stderr_task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    mut error: anyhow::Error,
) -> Result<anyhow::Error> {
    terminate_subscription(child).await?;
    let stderr = stderr_task.await.context("Claude stderr task failed")??;
    let stderr = String::from_utf8_lossy(&stderr);
    if !stderr.trim().is_empty() {
        error = error.context(format!("Claude subscription stderr: {}", stderr.trim()));
    }
    Ok(error)
}

pub(super) async fn validate_stream_exit(
    child: &mut Child,
    stderr_task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    saw_result: bool,
) -> Result<()> {
    let status = child.wait().await?;
    let stderr = stderr_task.await.context("Claude stderr task failed")??;
    if !status.success() {
        bail!(
            "Claude subscription exited with {status}: {}",
            String::from_utf8_lossy(&stderr).trim()
        );
    }
    if !saw_result {
        bail!("Claude subscription stream ended without a result event");
    }
    Ok(())
}

pub(super) async fn read_stderr(
    mut stderr: tokio::process::ChildStderr,
) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    stderr.read_to_end(&mut output).await?;
    Ok(output)
}
