use std::{sync::Arc, time::Duration};

use serde_json::Value;
use tokio::time::Instant;

use super::{
    SegmentBuilder, Session, StreamTurn, protocol::StreamSender,
    sanitize::is_visible_activity_event,
};

pub(in crate::anthropic) const ACTIVITY_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(4);
/// Main-turn callers sit blank until the first decoded event. Sub-second paint
/// keeps Claude Code / GPT / Codex hops feeling live without keepalive spam.
pub(in crate::anthropic) const INITIAL_ACTIVITY_DELAY: Duration = Duration::from_millis(250);
/// SubAgent TUI stays on Nucleating until the first keepalive/tool chrome.
pub(in crate::anthropic) const SUBAGENT_INITIAL_ACTIVITY_DELAY: Duration =
    Duration::from_millis(100);
/// Synthetic keepalives defeat Claude Code's ~600s stream watchdog. Bound
/// SubAgent silence (no visible provider events) so abandoned ACP work cannot
/// look "still running" for an hour. Any real provider activity resets this;
/// it is not a wall-clock hard kill from turn start.
pub(in crate::anthropic) const SUBAGENT_PROVIDER_SILENCE_JUDGMENT: Duration =
    Duration::from_secs(20 * 60);

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
    let interval = ACTIVITY_KEEPALIVE_INTERVAL;
    let initial = if is_subagent {
        SUBAGENT_INITIAL_ACTIVITY_DELAY
    } else {
        INITIAL_ACTIVITY_DELAY
    };
    (initial, interval)
}

pub(super) fn fail_if_subagent_provider_silent(builder: &SegmentBuilder) -> anyhow::Result<()> {
    if !builder.subagent_provider_silence_exceeded(SUBAGENT_PROVIDER_SILENCE_JUDGMENT) {
        return Ok(());
    }
    anyhow::bail!(
        "SubAgent provider produced no progress for {} seconds; ending the turn so Claude Code stops waiting on abandoned work",
        SUBAGENT_PROVIDER_SILENCE_JUDGMENT.as_secs()
    )
}
