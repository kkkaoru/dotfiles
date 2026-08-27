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
        Some(status) => bail!("ACP provider turn ended with status {status}"),
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
            "ACP provider is retrying the turn"
        );
        return Ok(ControlFlow::Continue(()));
    }
    if super::context_window::is_context_window_event(event) {
        tracing::warn!(error = %event.get("params").unwrap_or(event), "ACP provider hit context window limit");
    }
    if super::usage_limit::is_usage_limit_event(event) {
        tracing::warn!(error = %event.get("params").unwrap_or(event), "ACP provider hit usage limit");
    }
    if super::super::provider_auth::is_auth_failure_event(event) {
        tracing::warn!(error = %event.get("params").unwrap_or(event), "ACP provider hit provider auth failure");
    }
    bail!("{}", turn_error_message(event))
}

fn turn_error_message(event: &Value) -> String {
    let params = event.get("params").unwrap_or(event);
    let message = params
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("");
    if super::super::segment::contains_cline_credits_balance_marker(message)
        || super::super::segment::contains_cline_credits_balance_marker(&params.to_string())
    {
        return super::super::segment::cline_credits_failure_message(message);
    }
    format!("ACP provider turn failed: {params}")
}

pub(super) async fn refresh_activity_keepalive(
    builder: &SegmentBuilder,
    sender: Option<&StreamSender>,
    mut deadline: Pin<&mut Sleep>,
    interval: Duration,
) -> Result<()> {
    builder.activity_keepalive(sender).await?;
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "control_tests.rs"]
mod tests;
