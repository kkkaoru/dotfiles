use std::{convert::Infallible, pin::Pin, time::Duration};

use anyhow::Result;
use axum::body::Bytes;
use serde_json::Value;
use tokio::{
    io::{AsyncBufRead, Lines},
    sync::mpsc,
    time::Sleep,
};

use super::SubscriptionStream;
use crate::anthropic::subscription::failure;

/// After an Agent/Task launch, keep reading briefly so sibling launches in the
/// same subscription turn are forwarded before the SSE turn closes.
/// Absolute per open window (refreshed only on a new launch), not per hidden line.
pub(super) const LAUNCH_FANOUT_DRAIN: Duration = Duration::from_millis(250);

pub(super) enum StreamIteration {
    Continue,
    End,
    SenderClosed,
    EndEarly,
}

enum IterationReady {
    SenderClosed,
    Line(std::io::Result<Option<String>>),
    ActivityDeadline,
    LaunchFanoutDrain,
}

enum LineOutcome {
    End,
    Hidden,
    Visible,
    EndEarly,
}

pub(super) fn reset_activity_deadline(deadline: &mut Pin<Box<Sleep>>, interval: Duration) {
    deadline
        .as_mut()
        .reset(tokio::time::Instant::now() + interval);
}

pub(super) async fn consume_stream_iteration<R>(
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
    let drain_until = stream
        .launch_fanout_open
        .then_some(stream.launch_fanout_deadline)
        .flatten();
    let ready = if let Some(deadline) = drain_until {
        let mut drain_deadline = Box::pin(tokio::time::sleep_until(deadline));
        tokio::select! {
            biased;
            () = sender.closed() => IterationReady::SenderClosed,
            line = lines.next_line() => IterationReady::Line(line),
            () = drain_deadline.as_mut() => IterationReady::LaunchFanoutDrain,
        }
    } else {
        tokio::select! {
            biased;
            () = sender.closed() => IterationReady::SenderClosed,
            line = lines.next_line() => IterationReady::Line(line),
            () = activity_deadline.as_mut() => IterationReady::ActivityDeadline,
        }
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
        IterationReady::LaunchFanoutDrain => Ok(StreamIteration::EndEarly),
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
        LineOutcome::EndEarly => return Ok(StreamIteration::EndEarly),
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
        // Result ends the provider turn; do not keep the fan-out drain open.
        stream.clear_launch_fanout();
        return Ok(LineOutcome::Hidden);
    }
    let visible = stream.handle_envelope(sender, &envelope).await?;
    // Stop immediately on a blocked SubAgent or a non-launch tool that needs a
    // result round-trip. Agent/Task fan-out stays open so sibling launches in
    // later assistant events are still forwarded.
    if stream.blocked_subagent || (stream.saw_tool_use && !stream.launch_fanout_open) {
        return Ok(LineOutcome::EndEarly);
    }
    Ok(if visible {
        LineOutcome::Visible
    } else {
        LineOutcome::Hidden
    })
}
