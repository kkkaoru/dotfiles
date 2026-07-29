use anyhow::{Context, Result, bail};
use tokio::{io::AsyncReadExt, process::Child};

use super::super::subscription::terminate_subscription;

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
