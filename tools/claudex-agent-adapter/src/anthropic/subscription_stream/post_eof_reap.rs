use std::{convert::Infallible, future::pending, pin::Pin, process::ExitStatus, time::Duration};

use anyhow::{Context, Result};
use axum::body::Bytes;
use tokio::{process::Child, sync::mpsc, time::Sleep};

use super::{StderrTask, SubscriptionStream};
use crate::anthropic::{
    subscription::{SubscriptionOptions, terminate_subscription_process_group},
    subscription_stream::consume::reset_activity_deadline,
};

pub(in crate::anthropic) async fn cleanup_process_group(
    child: &mut Child,
    process_group: Option<u32>,
    stderr_task: &mut Option<StderrTask>,
    options: &SubscriptionOptions,
) -> Result<()> {
    let termination =
        terminate_subscription_process_group(child, process_group, options.termination_timeout)
            .await;
    let stderr = reap_stderr(stderr_task, options.termination_timeout).await;
    termination?;
    stderr?;
    Ok(())
}

pub(in crate::anthropic) async fn reap_stderr(
    task: &mut Option<StderrTask>,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let Some(mut task) = task.take() else {
        return Ok(Vec::new());
    };
    match tokio::time::timeout(timeout, &mut task).await {
        Ok(output) => output
            .context("Claude stderr task failed")?
            .map_err(Into::into),
        Err(_) => {
            task.abort();
            let _ = task.await;
            Ok(Vec::new())
        }
    }
}

pub(in crate::anthropic) async fn await_stderr(
    task: &mut Option<StderrTask>,
) -> Result<std::io::Result<Vec<u8>>, tokio::task::JoinError> {
    match task {
        Some(task) => task.await,
        None => pending().await,
    }
}

pub(in crate::anthropic) fn take_stderr(
    task: &mut Option<StderrTask>,
    output: Result<std::io::Result<Vec<u8>>, tokio::task::JoinError>,
) -> Result<Vec<u8>> {
    task.take();
    output
        .context("Claude stderr task failed")?
        .map_err(Into::into)
}

pub(in crate::anthropic) async fn keepalive(
    stream: &mut SubscriptionStream,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    activity_deadline: &mut Pin<Box<Sleep>>,
    options: &SubscriptionOptions,
) -> Result<()> {
    stream.activity_keepalive(sender).await?;
    reset_activity_deadline(activity_deadline, options.activity_keepalive_interval);
    Ok(())
}

pub(super) enum LeaderReady {
    SenderClosed,
    Status(std::io::Result<ExitStatus>),
    Stderr(Result<std::io::Result<Vec<u8>>, tokio::task::JoinError>),
    Activity,
}

pub(super) enum DrainReady {
    SenderClosed,
    Stderr(Result<std::io::Result<Vec<u8>>, tokio::task::JoinError>),
    GraceExpired,
    Activity,
}
