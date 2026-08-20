use std::sync::Arc;

use super::{StreamTurn, StreamWaitResult, builder::SegmentBuilder};
use crate::anthropic::{Bridge, Session};

mod helpers;
#[cfg(test)]
#[allow(unused_imports)]
use helpers::reject_disconnected_tool_once;
#[path = "disconnect_policy.rs"]
mod policy;
#[cfg(test)]
#[allow(unused_imports)]
pub(super) use helpers::drain_disconnected_turn;
#[cfg(test)]
use helpers::request_id_keys;

impl Bridge {
    pub(super) async fn finish_closed_stream(
        &self,
        session: &Arc<Session>,
        events: &Arc<crate::app_server::ThreadEvents>,
        provider_settled: bool,
    ) {
        if provider_settled {
            retain_settled_session_activity(session);
            return;
        }
        self.disconnect_stream(session, Arc::clone(events)).await;
    }
}

fn retain_settled_session_activity(session: &Session) {
    // Keep the idle provider thread so Claude Code SendMessage({to}) can
    // match the transcript and reuse prompt-cache prefixes. Capacity
    // pressure and IDLE_SESSION_TTL still reclaim these sessions.
    if let Ok(mut activity) = session.last_activity.lock() {
        *activity = std::time::Instant::now();
    }
    tracing::debug!(
        thread_id = %session.thread_id,
        "retaining settled session for SendMessage continue reuse"
    );
}

impl Bridge {
    /// Claude Code often drops SubAgent SSE right after `message_start`. Keep
    /// ACP alive in that window. Once ▶ tools, bridged tool_use, or other
    /// provider turn output (Status / answer chrome) exists, treat SSE close as
    /// stop/interrupt and cancel the provider leaf.
    pub(super) async fn subagent_sse_closed(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
        builder: &SegmentBuilder,
    ) -> StreamWaitResult {
        if builder.has_live_provider_work() {
            tracing::info!(
                thread_id = %session.thread_id,
                "SubAgent SSE disconnected after provider work; cancelling turn"
            );
            return StreamWaitResult::Done(Box::new(self.disconnect_stream(session, events).await));
        }
        tracing::info!(
            thread_id = %session.thread_id,
            "SubAgent SSE disconnected; continuing provider turn"
        );
        StreamWaitResult::NoEvent
    }

    pub(in crate::anthropic) async fn disconnect_stream(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
    ) -> StreamTurn {
        self.disconnect_stream_with_policy(session, events, true)
            .await
    }

    /// Disconnect a native background-Agent handoff without aborting the
    /// provider leaf. Routed Codex app-servers are shared by multiple
    /// sessions, so the generic visible-tool abort policy would close
    /// unrelated active streams and surface `event stream closed` errors.
    pub(in crate::anthropic) async fn disconnect_stream_for_async_handoff(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
    ) -> StreamTurn {
        self.disconnect_stream_with_policy(session, events, false)
            .await
    }
}

pub(super) fn warn_disconnect_failure(error: &anyhow::Error, thread_id: &str, message: &str) {
    tracing::warn!(%error, thread_id, message);
}

pub(super) fn warn_cancel_failure(error: &anyhow::Error, thread_id: &str) {
    warn_disconnect_failure(
        error,
        thread_id,
        "failed to cancel disconnected streaming turn",
    );
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "disconnect_tests.rs"]
mod tests;
