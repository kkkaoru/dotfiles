use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use super::Bridge;

#[cfg(test)]
use super::Session;
#[cfg(test)]
use tokio::sync::Mutex;

// An abandoned Claude tool request must not reserve a session slot forever.
// Thirty minutes allows long interactive tool work while bounding leaked slots.
pub(super) const PENDING_SESSION_TTL: Duration = Duration::from_secs(30 * 60);
// A session without pending tools can be reconstructed from Claude Code's next full transcript.
// Two hours preserves provider threads across substantial interactive pauses so related Agent and
// advisor follow-ups can reuse context and prompt prefixes. Capacity pressure can still evict the
// oldest idle session immediately, and pending or actively-owned sessions remain protected.
pub(super) const IDLE_SESSION_TTL: Duration = Duration::from_secs(120 * 60);
// Capacity pressure has its own immediate eviction path. Periodic sweeps only reclaim old idle
// transcripts, so scanning every session on every request wastes work during large Agent bursts.
pub(super) const SESSION_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

mod eviction;
use eviction::{drain_cancellation_responses, respond_to_evicted_request, session_sweep_due};
pub(crate) use eviction::{record_pending_tool, sweep_idle_sessions_at, take_oldest_evictable_at};

impl Bridge {
    pub(super) fn schedule_idle_session_sweep(self: &Arc<Self>) {
        let bridge = Arc::clone(self);
        tokio::spawn(async move {
            bridge.sweep_idle_sessions().await;
        });
    }

    pub(super) async fn sweep_idle_sessions(&self) {
        self.sweep_idle_sessions_if_due_at(Instant::now()).await;
    }

    async fn sweep_idle_sessions_if_due_at(&self, now: Instant) -> usize {
        if !session_sweep_due(&self.next_session_sweep, now) {
            return 0;
        }
        let removed = sweep_idle_sessions_at(&self.sessions, now).await;
        if removed > 0 {
            tracing::debug!(removed, "released idle claudex sessions");
        }
        removed
    }

    pub(super) async fn evict_oldest_idle_session(&self) {
        let Some(session) = take_oldest_evictable_at(&self.sessions, Instant::now()).await else {
            return;
        };
        for (request_id, result) in drain_cancellation_responses(&session).await {
            respond_to_evicted_request(self, &session.model, request_id, result).await;
        }
    }
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "retention_tests.rs"]
mod tests;
