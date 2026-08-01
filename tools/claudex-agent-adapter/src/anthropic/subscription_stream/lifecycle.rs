use std::{process::ExitStatus, time::Duration};

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::{io::AsyncReadExt, process::Child};

use super::super::subscription::{
    failure::{process_failure, protocol_failure},
    terminate_subscription_process_group,
};

pub(super) async fn terminate_after_stream_failure<T>(
    child: &mut Child,
    process_group: Option<u32>,
    termination_timeout: Duration,
    error: anyhow::Error,
) -> Result<T> {
    terminate_subscription_process_group(child, process_group, termination_timeout)
        .await
        .context("also failed to terminate the Claude subscription stream")?;
    Err(error)
}

pub(super) fn validate_stream_exit(
    status: &ExitStatus,
    stderr: &[u8],
    result: Option<&Value>,
    model: &str,
) -> Result<()> {
    if !status.success() {
        let stdout = result
            .and_then(|result| serde_json::to_vec(result).ok())
            .unwrap_or_default();
        return Err(process_failure(model, status, &stdout, stderr));
    }
    if result.is_none() {
        return Err(protocol_failure(
            Some(model),
            "stream ended without a result event",
        ));
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
