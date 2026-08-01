use std::{collections::HashSet, convert::Infallible, pin::Pin, time::Duration};

#[cfg(test)]
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::body::Bytes;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, BufReader, Lines},
    process::Child,
    sync::mpsc,
    time::Sleep,
};

#[cfg(test)]
use super::lifecycle::terminate_after_stream_failure;
use super::{
    SubscriptionStream,
    lifecycle::{read_stderr, validate_stream_exit},
    post_eof::await_post_eof,
};
use crate::anthropic::{
    subscription::{SubscriptionOptions, failure},
    subscription_activity::SubscriptionActivity,
};

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
            StreamIteration::Blocked => {
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

async fn consume_stream_iteration<R>(
    lines: &mut Lines<R>,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    model: &str,
    stream: &mut SubscriptionStream,
    pending_result: &mut Option<Value>,
    activity_deadline: &mut Pin<Box<Sleep>>,
    activity_keepalive_interval: Duration,
) -> Result<StreamIteration>
where
    R: AsyncBufRead + Unpin,
{
    // Prefer output lines over keepalives so an already-buffered provider event
    // is ordered first. Expired activity is still applied after a hidden line.
    let ready = tokio::select! {
        biased;
        () = sender.closed() => IterationReady::SenderClosed,
        line = lines.next_line() => IterationReady::Line(line),
        () = activity_deadline.as_mut() => IterationReady::ActivityDeadline,
    };
    match ready {
        IterationReady::SenderClosed => Ok(StreamIteration::SenderClosed),
        IterationReady::Line(line) => {
            let outcome = handle_next_line(line?, sender, model, stream, pending_result).await?;
            apply_line_outcome(
                outcome,
                sender,
                stream,
                activity_deadline,
                activity_keepalive_interval,
            )
            .await
        }
        IterationReady::ActivityDeadline => {
            stream.activity_keepalive(sender).await?;
            reset_activity_deadline(activity_deadline, activity_keepalive_interval);
            Ok(StreamIteration::Continue)
        }
    }
}

async fn apply_line_outcome(
    outcome: LineOutcome,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    stream: &mut SubscriptionStream,
    activity_deadline: &mut Pin<Box<Sleep>>,
    activity_keepalive_interval: Duration,
) -> Result<StreamIteration> {
    match outcome {
        LineOutcome::End => return Ok(StreamIteration::End),
        LineOutcome::Blocked => return Ok(StreamIteration::Blocked),
        LineOutcome::Visible => {
            reset_activity_deadline(activity_deadline, activity_keepalive_interval);
        }
        LineOutcome::Hidden if activity_deadline.deadline() <= tokio::time::Instant::now() => {
            stream.activity_keepalive(sender).await?;
            reset_activity_deadline(activity_deadline, activity_keepalive_interval);
        }
        LineOutcome::Hidden => {}
    }
    Ok(StreamIteration::Continue)
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
            StreamIteration::Blocked => {
                anyhow::bail!("test reader emitted a blocked SubAgent");
            }
        }
    }
    stream.activity.close(sender).await?;
    let result = pending_result.context("test reader ended without a result")?;
    stream.finish(sender, &result).await
}

impl SubscriptionStream {
    fn from_options(options: &SubscriptionOptions) -> Self {
        Self {
            text_started: false,
            text_closed: false,
            saw_tool_use: false,
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

async fn handle_next_line(
    line: Option<String>,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    model: &str,
    stream: &mut SubscriptionStream,
    pending_result: &mut Option<Value>,
) -> Result<LineOutcome> {
    let Some(line) = line else {
        return Ok(LineOutcome::End);
    };
    if pending_result.is_some() {
        return Ok(LineOutcome::Hidden);
    }
    let envelope = failure::parse_stream_envelope(Some(model), &line)?;
    if envelope.get("type").and_then(Value::as_str) == Some("result") {
        *pending_result = Some(envelope);
        return Ok(LineOutcome::Hidden);
    }
    let visible = stream.handle_envelope(sender, &envelope).await?;
    if stream.blocked_subagent {
        return Ok(LineOutcome::Blocked);
    }
    Ok(if visible {
        LineOutcome::Visible
    } else {
        LineOutcome::Hidden
    })
}

enum LineOutcome {
    End,
    Hidden,
    Visible,
    Blocked,
}

enum IterationReady {
    SenderClosed,
    Line(std::io::Result<Option<String>>),
    ActivityDeadline,
}

enum StreamIteration {
    Continue,
    End,
    SenderClosed,
    Blocked,
}

async fn finish_blocked_stream(
    child: &mut Child,
    process_group: Option<u32>,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    stderr_task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    mut stream: SubscriptionStream,
    options: &SubscriptionOptions,
) -> Result<()> {
    super::post_eof::cleanup_closed_stream(child, process_group, stderr_task, options).await?;
    stream
        .finish(
            sender,
            &json!({"type":"result", "subtype":"success", "is_error":false, "result":""}),
        )
        .await
}

async fn finish_stream(
    child: &mut Child,
    stderr_task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    mut stream: SubscriptionStream,
    pending_result: Option<Value>,
    context: FinishStreamContext<'_>,
) -> Result<()> {
    let Some(output) = await_post_eof(
        child,
        context.process_group,
        stderr_task,
        context.sender,
        context.options,
        &mut stream,
        context.activity_deadline,
    )
    .await?
    else {
        return Ok(());
    };
    stream.activity.close(context.sender).await?;
    if let Err(error) = validate_stream_exit(
        &output.status,
        &output.stderr,
        pending_result.as_ref(),
        context.model,
    ) {
        return Err(stream_error(&mut stream, context.sender, context.model, error).await);
    }
    let result = pending_result.expect("validated stream result is present");
    if let Err(error) = stream.finish(context.sender, &result).await {
        return Err(stream_error(&mut stream, context.sender, context.model, error).await);
    }
    Ok(())
}

struct FinishStreamContext<'a> {
    process_group: Option<u32>,
    sender: &'a mpsc::Sender<Result<Bytes, Infallible>>,
    model: &'a str,
    options: &'a SubscriptionOptions,
    activity_deadline: &'a mut Pin<Box<Sleep>>,
}

async fn stream_error(
    stream: &mut SubscriptionStream,
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    model: &str,
    error: anyhow::Error,
) -> anyhow::Error {
    let _ = stream.close_text(sender).await;
    if stream.next_index > 0 {
        failure::after_stream_output(model, error)
    } else {
        error
    }
}

pub(super) fn reset_activity_deadline(deadline: &mut Pin<Box<Sleep>>, interval: Duration) {
    deadline
        .as_mut()
        .reset(tokio::time::Instant::now() + interval);
}
