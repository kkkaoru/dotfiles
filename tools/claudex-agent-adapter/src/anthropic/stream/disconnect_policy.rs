use std::{collections::HashSet, sync::Arc};

use super::{
    helpers::{
        drain_disconnected_turn_with_warning, reject_disconnected_tool_with_warning,
        request_id_keys, take_pending_disconnected_tools,
    },
    warn_cancel_failure, warn_disconnect_failure,
};
use crate::{
    agent_backend::TurnCancellation,
    anthropic::{Bridge, Session, stream::StreamTurn},
};

impl Bridge {
pub(super) async fn disconnect_stream_with_policy(
    &self,
    session: &Arc<Session>,
    events: Arc<crate::app_server::ThreadEvents>,
    abort_visible_tool_provider: bool,
) -> StreamTurn {
    // Cancel before unregistering so a racing outer follow-up can still
    // discover this session, preempt the gate, and reuse the provider thread.
    match self.app.cancel_turn(&session.thread_id).await {
        Ok(TurnCancellation::Settled) => {
            let _ = self.reject_pending_disconnected_tools(session).await;
            self.remove_session(session).await;
        }
        Ok(TurnCancellation::Unsupported) => {
            self.handle_unsupported_disconnect(session, events, abort_visible_tool_provider)
                .await;
        }
        Err(error) => {
            warn_cancel_failure(&error, &session.thread_id);
            self.detach_non_cancellable_turn(session, events).await;
            self.remove_session(session).await;
        }
    }
    StreamTurn::Disconnected
}

pub(super) async fn handle_unsupported_disconnect(
    &self,
    session: &Arc<Session>,
    events: Arc<crate::app_server::ThreadEvents>,
    abort_visible_tool_provider: bool,
) {
    if session.pending_tools.lock().await.is_empty() {
        // No tool call has reached Claude Code yet. Keep consuming the
        // non-cancellable turn so a delayed call receives a rejection.
        self.detach_non_cancellable_turn(session, events).await;
        self.remove_session(session).await;
        return;
    }
    if !abort_visible_tool_provider {
        // A native Agent handoff already returned control to Claude Code.
        // Reject the pending call and drain only this turn so a shared
        // provider remains available to unrelated sessions.
        self.detach_non_cancellable_turn(session, events).await;
        self.remove_session(session).await;
        return;
    }
    // A client-visible tool call can no longer receive a result. Abort and
    // reap the provider instead of leaving hidden work attached to it.
    self.remove_session(session).await;
    self.discard_pending_disconnected_tools(session).await;
    self.abort_disconnected_provider(&session.thread_id).await;
}

pub(super) async fn abort_disconnected_provider(&self, thread_id: &str) {
    if let Err(error) = self.app.abort_turn_provider(thread_id).await {
        warn_disconnect_failure(
            &error,
            thread_id,
            "failed to abort non-cancellable disconnected provider",
        );
    }
}

pub(super) async fn detach_non_cancellable_turn(
    &self,
    session: &Arc<Session>,
    events: Arc<crate::app_server::ThreadEvents>,
) {
    let rejected_request_ids = self.reject_pending_disconnected_tools(session).await;
    self.spawn_disconnected_turn_drain(session.model.clone(), events, rejected_request_ids);
}

pub(super) fn spawn_disconnected_turn_drain(
    &self,
    model: String,
    events: Arc<crate::app_server::ThreadEvents>,
    rejected_request_ids: HashSet<String>,
) {
    let app = Arc::clone(&self.app);
    tokio::spawn(drain_disconnected_turn_with_warning(
        app,
        model,
        events,
        rejected_request_ids,
    ));
}

pub(super) async fn reject_pending_disconnected_tools(&self, session: &Session) -> HashSet<String> {
    let pending = take_pending_disconnected_tools(session).await;
    self.agent_efforts
        .remove_tool_results(pending.iter().map(|(tool_use_id, _)| tool_use_id.as_str()));
    let rejected_request_ids = request_id_keys(&pending);
    for (_, request_id) in pending {
        reject_disconnected_tool_with_warning(self, session, request_id).await;
    }
    rejected_request_ids
}

/// Pure mid-turn follow-ups reclaim a session that still owns Claude tool
/// calls from the prior segment. Reject those calls so the provider can
/// accept the new user turn instead of waiting forever for tool_result.
pub(in crate::anthropic) async fn settle_abandoned_pending_tools(&self, session: &Session) {
    if session.pending_tools.lock().await.is_empty() {
        return;
    }
    tracing::info!(
        thread_id = %session.thread_id,
        "settling abandoned pending tools before pure mid-turn follow-up"
    );
    let _ = self.reject_pending_disconnected_tools(session).await;
}

pub(super) async fn discard_pending_disconnected_tools(&self, session: &Session) {
    let pending = take_pending_disconnected_tools(session).await;
    self.agent_efforts
        .remove_tool_results(pending.iter().map(|(tool_use_id, _)| tool_use_id.as_str()));
}
}
