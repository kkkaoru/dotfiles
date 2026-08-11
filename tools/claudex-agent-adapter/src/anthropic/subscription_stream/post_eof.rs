use std::{convert::Infallible, pin::Pin, process::ExitStatus};

use anyhow::Result;
use axum::body::Bytes;
use tokio::{process::Child, sync::mpsc, task::JoinHandle, time::Sleep};

use super::SubscriptionStream;
use crate::anthropic::{
    subscription::{SubscriptionOptions, terminate_subscription_process_group},
};

pub(super) type StderrTask = JoinHandle<std::io::Result<Vec<u8>>>;

#[path = "post_eof_reap.rs"]
mod reap;
use reap::{DrainReady, LeaderReady, cleanup_process_group, keepalive};
pub(in crate::anthropic) use reap::{await_stderr, reap_stderr, take_stderr};

pub(super) struct PostEofOutput {
    pub(super) status: ExitStatus,
    pub(super) stderr: Vec<u8>,
}

pub(super) async fn cleanup_closed_stream(
    child: &mut Child,
    process_group: Option<u32>,
    stderr_task: StderrTask,
    options: &SubscriptionOptions,
) -> Result<()> {
    let mut stderr_task = Some(stderr_task);
    cleanup_process_group(child, process_group, &mut stderr_task, options).await
}

pub(super) async fn await_post_eof(
    child: &mut Child,
    process_group: Option<u32>,
    stderr_task: StderrTask,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    options: &SubscriptionOptions,
    stream: &mut SubscriptionStream,
    activity_deadline: &mut Pin<Box<Sleep>>,
) -> Result<Option<PostEofOutput>> {
    let mut stderr_task = Some(stderr_task);
    let mut stderr = None;
    let status = loop {
        let ready = tokio::select! {
            biased;
            () = sender.closed() => LeaderReady::SenderClosed,
            status = child.wait() => LeaderReady::Status(status),
            output = await_stderr(&mut stderr_task) => LeaderReady::Stderr(output),
            () = activity_deadline.as_mut() => LeaderReady::Activity,
        };
        match ready {
            LeaderReady::SenderClosed => {
                cleanup_process_group(child, process_group, &mut stderr_task, options).await?;
                return Ok(None);
            }
            LeaderReady::Status(status) => break status?,
            LeaderReady::Stderr(output) => stderr = Some(take_stderr(&mut stderr_task, output)?),
            LeaderReady::Activity => keepalive(stream, sender, activity_deadline, options).await?,
        }
    };
    let stderr = match stderr {
        Some(stderr) => stderr,
        None => {
            let Some(stderr) = drain_stderr_after_exit(
                child,
                process_group,
                &mut stderr_task,
                sender,
                options,
                stream,
                activity_deadline,
            )
            .await?
            else {
                return Ok(None);
            };
            stderr
        }
    };
    Ok(Some(PostEofOutput { status, stderr }))
}

async fn drain_stderr_after_exit(
    child: &mut Child,
    process_group: Option<u32>,
    stderr_task: &mut Option<StderrTask>,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    options: &SubscriptionOptions,
    stream: &mut SubscriptionStream,
    activity_deadline: &mut Pin<Box<Sleep>>,
) -> Result<Option<Vec<u8>>> {
    let grace = tokio::time::sleep(options.stderr_drain_grace);
    tokio::pin!(grace);
    loop {
        let ready = tokio::select! {
            biased;
            () = sender.closed() => DrainReady::SenderClosed,
            output = await_stderr(stderr_task) => DrainReady::Stderr(output),
            () = grace.as_mut() => DrainReady::GraceExpired,
            () = activity_deadline.as_mut() => DrainReady::Activity,
        };
        match ready {
            DrainReady::SenderClosed => {
                cleanup_process_group(child, process_group, stderr_task, options).await?;
                return Ok(None);
            }
            DrainReady::Stderr(output) => return take_stderr(stderr_task, output).map(Some),
            DrainReady::GraceExpired => {
                terminate_subscription_process_group(
                    child,
                    process_group,
                    options.termination_timeout,
                )
                .await?;
                return reap_stderr(stderr_task, options.termination_timeout)
                    .await
                    .map(Some);
            }
            DrainReady::Activity => keepalive(stream, sender, activity_deadline, options).await?,
        }
    }
}

