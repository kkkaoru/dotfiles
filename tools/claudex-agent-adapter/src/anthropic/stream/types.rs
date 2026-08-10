use std::{sync::Arc, time::Duration};

use serde_json::Value;
use tokio::time::Instant;

use super::{
    SegmentBuilder, Session, StreamTurn, protocol::StreamSender,
    sanitize::is_visible_activity_event,
};

pub(in crate::anthropic) struct ToolCall {
    pub(in crate::anthropic::stream) call_id: String,
    pub(in crate::anthropic::stream) name: String,
    pub(in crate::anthropic::stream) arguments: Value,
    pub(in crate::anthropic::stream) request_id: Value,
}

pub(in crate::anthropic) struct StreamWaitInput<'a> {
    pub(in crate::anthropic::stream) session: &'a Arc<Session>,
    pub(in crate::anthropic::stream) events: Arc<crate::app_server::ThreadEvents>,
    pub(in crate::anthropic::stream) current_messages: &'a [Value],
    pub(in crate::anthropic::stream) system: &'a Value,
    pub(in crate::anthropic::stream) sender: &'a StreamSender,
    pub(in crate::anthropic::stream) builder: SegmentBuilder,
    pub(in crate::anthropic::stream) activity_interval: Duration,
    /// First silence before keepalive; SubAgents use a shorter delay than the
    /// steady `activity_interval` so mid-turn provider quiet does not freeze
    /// Claude Code on Nucleating / Thought-for chrome.
    pub(in crate::anthropic::stream) initial_activity_delay: Duration,
}

pub(in crate::anthropic::stream) enum StreamWaitResult {
    Event(Value),
    Done(Box<StreamTurn>),
    NoEvent,
}

pub(in crate::anthropic::stream) enum StreamEventState {
    Continue,
    Done(Box<StreamTurn>),
    ContextWindow(anyhow::Error),
    UsageLimit(anyhow::Error),
}

pub(in crate::anthropic) fn is_provider_stream_closed(error: &anyhow::Error) -> bool {
    error.to_string().contains("app-server event stream closed")
}

pub(in crate::anthropic::stream) fn reset_activity_deadline(
    event: &Value,
    deadline: &mut std::pin::Pin<Box<tokio::time::Sleep>>,
    interval: Duration,
) {
    if is_visible_activity_event(event) {
        deadline.as_mut().reset(Instant::now() + interval);
    }
}

pub(in crate::anthropic::stream) fn stream_activity_delays(
    is_subagent: bool,
) -> (Duration, Duration) {
    let interval = super::ACTIVITY_KEEPALIVE_INTERVAL;
    let initial = if is_subagent {
        super::SUBAGENT_INITIAL_ACTIVITY_DELAY
    } else {
        interval
    };
    (initial, interval)
}
