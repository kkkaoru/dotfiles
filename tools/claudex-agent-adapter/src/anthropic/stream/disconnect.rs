use std::{collections::HashSet, ops::ControlFlow, sync::Arc};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{
    StreamTurn, StreamWaitResult, builder::SegmentBuilder, builder::parse_tool_call, error_flow,
    turn_flow,
};
use crate::anthropic::content::pending_request_id;
use crate::{
    agent_backend::TurnCancellation,
    anthropic::{Bridge, Session},
};

impl Bridge {
    pub(super) async fn finish_closed_stream(
        &self,
        session: &Arc<Session>,
        events: &Arc<crate::app_server::ThreadEvents>,
        provider_settled: bool,
    ) {
        if provider_settled {
            // Keep the idle provider thread so Claude Code Task `resume` can
            // match the transcript and reuse prompt-cache prefixes. Capacity
            // pressure and IDLE_SESSION_TTL still reclaim these sessions.
            if let Ok(mut activity) = session.last_activity.lock() {
                *activity = std::time::Instant::now();
            }
            tracing::debug!(
                thread_id = %session.thread_id,
                "retaining settled session for SubAgent resume reuse"
            );
        } else {
            self.disconnect_stream(session, Arc::clone(events)).await;
        }
    }

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
            return StreamWaitResult::Done(Box::new(
                self.disconnect_stream(session, events).await,
            ));
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

    async fn disconnect_stream_with_policy(
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

    async fn handle_unsupported_disconnect(
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

    async fn abort_disconnected_provider(&self, thread_id: &str) {
        if let Err(error) = self.app.abort_turn_provider(thread_id).await {
            warn_disconnect_failure(
                &error,
                thread_id,
                "failed to abort non-cancellable disconnected provider",
            );
        }
    }

    async fn detach_non_cancellable_turn(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
    ) {
        let rejected_request_ids = self.reject_pending_disconnected_tools(session).await;
        self.spawn_disconnected_turn_drain(session.model.clone(), events, rejected_request_ids);
    }

    fn spawn_disconnected_turn_drain(
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

    async fn reject_pending_disconnected_tools(&self, session: &Session) -> HashSet<String> {
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

    async fn discard_pending_disconnected_tools(&self, session: &Session) {
        let pending = take_pending_disconnected_tools(session).await;
        self.agent_efforts
            .remove_tool_results(pending.iter().map(|(tool_use_id, _)| tool_use_id.as_str()));
    }
}

async fn drain_disconnected_turn_with_warning(
    app: Arc<crate::agent_backend::AgentBackend>,
    model: String,
    events: Arc<crate::app_server::ThreadEvents>,
    rejected_request_ids: HashSet<String>,
) {
    if let Err(error) = drain_disconnected_turn(&app, &model, events, rejected_request_ids).await {
        warn_disconnect_failure(
            &error,
            &model,
            "failed to drain disconnected non-cancellable turn",
        );
    }
}

async fn reject_disconnected_tool_with_warning(
    bridge: &Bridge,
    session: &Session,
    request_id: Value,
) {
    if crate::anthropic::stream::acp_tool_bridge::is_acp_bridge_request_id(&request_id) {
        return;
    }
    if let Err(error) = reject_disconnected_tool(&bridge.app, &session.model, request_id).await {
        warn_disconnect_failure(
            &error,
            &session.thread_id,
            "failed to reject pending tool after client disconnect",
        );
    }
}

pub(super) async fn drain_disconnected_turn(
    app: &crate::agent_backend::AgentBackend,
    model: &str,
    events: Arc<crate::app_server::ThreadEvents>,
    mut rejected_request_ids: HashSet<String>,
) -> Result<()> {
    loop {
        let event = events
            .recv()
            .await
            .context("app-server event stream closed while draining disconnected turn")?;
        match event.get("method").and_then(Value::as_str) {
            Some("item/tool/call") => {
                let request_id = parse_tool_call(&event)?.request_id;
                reject_disconnected_tool_once(app, model, &mut rejected_request_ids, request_id)
                    .await?;
            }
            Some("error") => {
                let _ = error_flow(&event)?;
            }
            Some("turn/completed") if turn_flow(&event)? == ControlFlow::Break(()) => {
                return Ok(());
            }
            _ => {}
        }
    }
}

async fn take_pending_disconnected_tools(session: &Session) -> Vec<(String, Value)> {
    let mut pending = session.pending_tools.lock().await;
    let mut tools = pending
        .drain()
        .map(|(tool_use_id, request_id)| (tool_use_id, pending_request_id(&request_id)))
        .collect::<Vec<_>>();
    tools.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    *session
        .pending_since
        .lock()
        .expect("pending tool clock poisoned") = None;
    tools
}

fn request_id_keys(tools: &[(String, Value)]) -> HashSet<String> {
    tools
        .iter()
        .map(|(_, request_id)| {
            serde_json::to_string(request_id).expect("tool request id is serializable")
        })
        .collect()
}

async fn reject_disconnected_tool_once(
    app: &crate::agent_backend::AgentBackend,
    model: &str,
    rejected_request_ids: &mut HashSet<String>,
    request_id: Value,
) -> Result<()> {
    let key = serde_json::to_string(&request_id).context("tool request id is not serializable")?;
    if rejected_request_ids.insert(key) {
        reject_disconnected_tool(app, model, request_id).await?;
    }
    Ok(())
}

async fn reject_disconnected_tool(
    app: &crate::agent_backend::AgentBackend,
    model: &str,
    request_id: Value,
) -> Result<()> {
    app.respond_for_model(
        model,
        request_id,
        json!({
            "contentItems":[{
                "type":"inputText",
                "text":"Claude Code disconnected before returning this tool result."
            }],
            "success":false
        }),
    )
    .await
    .context("failed to reject a tool call from a disconnected turn")
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
mod tests {
    use super::*;

    #[test]
    fn serializes_pending_request_id_keys_without_reordering_tools() {
        let keys = request_id_keys(&[
            ("first".to_owned(), json!(41)),
            ("second".to_owned(), json!({"id":"42"})),
        ]);
        assert!(keys.contains("41"));
        assert!(keys.contains(r#"{"id":"42"}"#));
    }

    #[tokio::test]
    async fn rejects_each_request_id_once_and_reports_provider_errors() {
        let app =
            crate::agent_backend::AgentBackend::Grok(crate::grok_acp::GrokAcp::stopped_for_test());
        let mut rejected = HashSet::new();
        assert!(
            reject_disconnected_tool_once(&app, "model", &mut rejected, json!(41))
                .await
                .is_err()
        );
        assert!(
            reject_disconnected_tool_once(&app, "model", &mut rejected, json!(41))
                .await
                .is_ok()
        );
        assert_eq!(rejected.len(), 1);
    }
}
