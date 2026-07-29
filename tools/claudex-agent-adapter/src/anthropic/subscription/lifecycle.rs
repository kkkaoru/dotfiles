use std::process::{Command as StdCommand, Output, Stdio};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, ChildStdin},
};

use super::write_subscription_prompt;

pub(super) async fn collect_subscription_output(
    child: &mut Child,
    stdin: ChildStdin,
    prompt: &str,
) -> Result<(Result<()>, Output)> {
    let stdout = child
        .stdout
        .take()
        .context("Claude subscription stdout is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("Claude subscription stderr is unavailable")?;
    let (prompt_result, status, stdout, stderr) = tokio::join!(
        write_subscription_prompt(stdin, prompt),
        child.wait(),
        read_output(stdout),
        read_output(stderr),
    );
    Ok((
        prompt_result,
        Output {
            status: status?,
            stdout: stdout?,
            stderr: stderr?,
        },
    ))
}

pub(super) async fn terminate_after_subscription_failure<T>(
    child: &mut Child,
    error: anyhow::Error,
) -> Result<T> {
    terminate_subscription(child)
        .await
        .context("also failed to terminate the Claude subscription")?;
    Err(error)
}

pub(in crate::anthropic) async fn terminate_subscription(child: &mut Child) -> Result<()> {
    let process_group = child.id();
    if child.try_wait()?.is_none() {
        let _ = child.start_kill();
    }
    if let Some(process_group) = process_group {
        terminate_process_group(process_group);
    }
    child
        .wait()
        .await
        .context("wait for terminated Claude subscription")?;
    Ok(())
}

async fn read_output<T>(mut output: T) -> std::io::Result<Vec<u8>>
where
    T: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    output.read_to_end(&mut bytes).await?;
    Ok(bytes)
}

#[cfg(unix)]
fn terminate_process_group(process_group: u32) {
    let _status = StdCommand::new("kill")
        .args(["-KILL", &format!("-{process_group}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn terminate_process_group(_process_group: u32) {}

#[cfg(all(test, unix))]
mod tests {
    use std::{path::Path, sync::Arc, time::Duration};

    use super::{
        super::{OutputMode, SubscriptionOptions, spawn_subscription, subscription_command},
        terminate_after_subscription_failure,
    };

    #[cfg(unix)]
    #[tokio::test]
    async fn subscription_failure_after_exit_preserves_the_original_error() {
        let options = SubscriptionOptions::internal(
            Arc::new(tokio::sync::Semaphore::new(1)),
            Duration::from_secs(1),
        );
        let mut command =
            subscription_command(Path::new("true"), "model", &options, OutputMode::Json);
        let mut child =
            spawn_subscription(&mut command, "model").expect("spawn completed subscription");
        assert!(
            child
                .wait()
                .await
                .expect("wait for completed subscription")
                .success()
        );

        let error = terminate_after_subscription_failure::<()>(
            &mut child,
            anyhow::anyhow!("original subscription failure"),
        )
        .await
        .expect_err("the original failure is returned after reaping an exited child");

        assert_eq!(error.to_string(), "original subscription failure");
        assert!(child.try_wait().expect("inspect reaped child").is_some());
    }
}
