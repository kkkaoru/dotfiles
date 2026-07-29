use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::time::Duration;
use std::{ops::ControlFlow, pin::Pin};

use tokio::time::{Instant, Sleep};

use super::{Segment, Session, StreamSender, builder::SegmentBuilder};

pub(in crate::anthropic) fn turn_flow(event: &Value) -> Result<ControlFlow<()>> {
    match event.pointer("/params/turn/status").and_then(Value::as_str) {
        Some("completed") | None => Ok(ControlFlow::Break(())),
        Some("inProgress") => Ok(ControlFlow::Continue(())),
        Some(status) => bail!("codex app-server turn ended with status {status}"),
    }
}

pub(in crate::anthropic) fn error_flow(event: &Value) -> Result<ControlFlow<()>> {
    if event
        .pointer("/params/willRetry")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        tracing::warn!(
            error = %event.get("params").unwrap_or(event),
            "codex app-server is retrying the turn"
        );
        return Ok(ControlFlow::Continue(()));
    }
    if super::context_window::is_context_window_event(event) {
        tracing::warn!(error = %event.get("params").unwrap_or(event), "codex app-server hit context window limit");
    }
    bail!(
        "codex app-server turn failed: {}",
        event.get("params").unwrap_or(event)
    )
}

pub(super) async fn refresh_activity_keepalive(
    builder: &mut SegmentBuilder,
    sender: &StreamSender,
    mut deadline: Pin<&mut Sleep>,
    interval: Duration,
) -> Result<()> {
    builder.activity_keepalive(Some(sender)).await?;
    deadline.as_mut().reset(Instant::now() + interval);
    Ok(())
}

pub(in crate::anthropic) async fn commit_transcript(
    session: &Session,
    extras: Vec<Value>,
    segment: &Segment,
) {
    let mut transcript = session.transcript.lock().await;
    transcript.extend(extras);
    transcript.push(json!({"role":"assistant","content":segment.blocks}));
}
