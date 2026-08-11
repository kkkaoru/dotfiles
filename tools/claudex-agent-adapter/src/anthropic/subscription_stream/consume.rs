use std::{collections::HashSet, convert::Infallible};

#[cfg(test)]
use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use axum::body::Bytes;
#[cfg(test)]
use serde_json::json;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Child,
    sync::mpsc,
};

#[cfg(test)]
use tokio::io::AsyncBufRead;

use super::consume_fanout::{StreamIteration, consume_stream_iteration};
#[cfg(test)]
use super::lifecycle::terminate_after_stream_failure;
use super::{
    SubscriptionStream,
    lifecycle::read_stderr,
};
use crate::anthropic::{
    subscription::SubscriptionOptions,
    subscription_activity::SubscriptionActivity,
};

pub(super) use super::consume_fanout::reset_activity_deadline;
use super::consume_finish::{FinishStreamContext, finish_blocked_stream, finish_stream};

#[cfg(test)]
pub(super) async fn consume_subscription_stream(
    mut child: Child,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
) -> Result<()> {
    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(1),
    );
    let process_group = child.id();
    let result =
        consume_subscription_stream_with_options(&mut child, sender, &options, "subscription-test")
            .await;
    if let Err(error) = result {
        return terminate_after_stream_failure(
            &mut child,
            process_group,
            options.termination_timeout,
            error,
        )
        .await;
    }
    Ok(())
}

pub(super) async fn consume_subscription_stream_with_options(
    child: &mut Child,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    options: &SubscriptionOptions,
    model: &str,
) -> Result<()> {
    let process_group = child.id();
    let stdout = child
        .stdout
        .take()
        .context("Claude subscription stdout is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("Claude subscription stderr is unavailable")?;
    let stderr_task = tokio::spawn(read_stderr(stderr));
    let mut lines = BufReader::new(stdout).lines();
    let mut stream = SubscriptionStream::from_options(options);
    stream
        .start_subagent_activity(sender, options, model)
        .await?;
    let mut pending_result = None;
    let mut activity_deadline = Box::pin(tokio::time::sleep(options.initial_activity_delay));
    loop {
        let iteration = consume_stream_iteration(
            &mut lines,
            sender,
            model,
            &mut stream,
            &mut pending_result,
            &mut activity_deadline,
            options.activity_keepalive_interval,
        )
        .await?;
        match iteration {
            StreamIteration::Continue => {}
            StreamIteration::End => break,
            StreamIteration::SenderClosed => {
                return super::post_eof::cleanup_closed_stream(
                    child,
                    process_group,
                    stderr_task,
                    options,
                )
                .await;
            }
            StreamIteration::EndEarly => {
                return finish_blocked_stream(
                    child,
                    process_group,
                    sender,
                    stderr_task,
                    stream,
                    options,
                )
                .await;
            }
        }
    }
    finish_stream(
        child,
        stderr_task,
        stream,
        pending_result,
        FinishStreamContext {
            process_group,
            sender,
            model,
            options,
            activity_deadline: &mut activity_deadline,
        },
    )
    .await
}

#[cfg(test)]
impl SubscriptionStream {
    pub(crate) async fn consume_reader_for_test<R>(
        reader: R,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        options: &SubscriptionOptions,
        model: &str,
    ) -> Result<()>
    where
        R: AsyncBufRead + Unpin,
    {
        consume_test_reader(reader, sender, options, model).await
    }
}

#[cfg(test)]
async fn consume_test_reader<R>(
    reader: R,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    options: &SubscriptionOptions,
    model: &str,
) -> Result<()>
where
    R: AsyncBufRead + Unpin,
{
    let mut lines = reader.lines();
    let mut stream = SubscriptionStream::from_options(options);
    stream
        .start_subagent_activity(sender, options, model)
        .await?;
    let mut pending_result = None;
    let mut activity_deadline = Box::pin(tokio::time::sleep(options.initial_activity_delay));
    loop {
        let iteration = consume_stream_iteration(
            &mut lines,
            sender,
            model,
            &mut stream,
            &mut pending_result,
            &mut activity_deadline,
            options.activity_keepalive_interval,
        )
        .await?;
        match iteration {
            StreamIteration::Continue => {}
            StreamIteration::End => break,
            StreamIteration::SenderClosed => return Ok(()),
            StreamIteration::EndEarly => {
                finish_end_early(sender, &mut stream).await?;
                return Ok(());
            }
        }
    }
    stream.activity.close(sender).await?;
    let result = pending_result.context("test reader ended without a result")?;
    stream.finish(sender, &result).await
}

#[cfg(test)]
async fn finish_end_early(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    stream: &mut SubscriptionStream,
) -> Result<()> {
    if stream.blocked_subagent {
        anyhow::bail!("test reader emitted a blocked SubAgent");
    }
    stream.activity.close(sender).await?;
    stream
        .finish(
            sender,
            &json!({"type":"result","subtype":"success","is_error":false,"result":""}),
        )
        .await
}

impl SubscriptionStream {
    fn from_options(options: &SubscriptionOptions) -> Self {
        Self {
            text_started: false,
            text_closed: false,
            saw_tool_use: false,
            launch_fanout_open: false,
            seen_tool_ids: HashSet::new(),
            blocked_subagent: false,
            saw_result: false,
            next_index: 0,
            tools: options.tools.clone(),
            tool_context: options.tool_context.clone(),
            activity: SubscriptionActivity::default(),
        }
    }
}
