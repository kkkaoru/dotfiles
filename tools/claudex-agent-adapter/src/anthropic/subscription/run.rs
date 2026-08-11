use anyhow::{Context, Result, anyhow};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, ChildStdin, Command},
    sync::{OwnedSemaphorePermit, Semaphore},
};

use super::{
    OutputMode, SubscriptionOptions,
    failure::{
        local_failure, local_result, process_failure, spawn_child, subscription_result_for_model,
        timeout_failure,
    },
    lifecycle::{collect_subscription_output, terminate_after_subscription_failure},
    retry, subscription_command,
};

pub(in crate::anthropic) async fn run_subscription_model(
    program: &Path,
    model: &str,
    prompt: &str,
    options: SubscriptionOptions,
) -> Result<String> {
    retry::with_transient_retries(model, || {
        run_subscription_model_attempt(program, model, prompt, &options)
    })
    .await
}

async fn run_subscription_model_attempt(
    program: &Path,
    model: &str,
    prompt: &str,
    options: &SubscriptionOptions,
) -> Result<String> {
    let _permit = acquire_subscription_slot(Arc::clone(&options.slots), options.timeout).await?;
    let mut command = subscription_command(program, model, options, OutputMode::Json);
    let (mut child, stdin) = spawn_child(&mut command, model)?;
    let interaction = collect_subscription_output(&mut child, stdin, prompt);
    let (prompt_result, output) = match tokio::time::timeout(options.timeout, interaction).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            let error = local_failure(model, "failed to collect child output", &error);
            return terminate_after_subscription_failure(&mut child, error).await;
        }
        Err(_) => {
            let error = timeout_failure(model, options.timeout);
            return terminate_after_subscription_failure(&mut child, error).await;
        }
    };
    if !output.status.success() {
        return Err(process_failure(
            model,
            &output.status,
            &output.stdout,
            &output.stderr,
        ));
    }
    local_result(model, "failed to write prompt", prompt_result)?;
    subscription_result_for_model(&output.stdout, Some(model))
}

pub(in crate::anthropic) async fn acquire_subscription_slot(
    slots: Arc<Semaphore>,
    timeout: Duration,
) -> Result<OwnedSemaphorePermit> {
    tokio::time::timeout(timeout, slots.acquire_owned())
        .await
        .map_err(|_| anyhow!("Claude subscription capacity wait timed out"))?
        .map_err(|_| anyhow!("Claude subscription capacity is closed"))
}

pub(in crate::anthropic) fn spawn_subscription(
    command: &mut Command,
    model: &str,
) -> Result<Child> {
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to start Claude subscription model {model}"))
}

pub(in crate::anthropic) fn take_subscription_stdin(child: &mut Child) -> Result<ChildStdin> {
    child
        .stdin
        .take()
        .context("Claude subscription stdin is unavailable")
}

pub(in crate::anthropic) async fn write_subscription_prompt(
    mut stdin: ChildStdin,
    prompt: &str,
) -> Result<()> {
    stdin.write_all(prompt.as_bytes()).await?;
    stdin.shutdown().await.map_err(Into::into) // explicit EOF for --print
}
