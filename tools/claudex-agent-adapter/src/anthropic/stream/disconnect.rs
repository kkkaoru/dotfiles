use std::{collections::HashSet, ops::ControlFlow, sync::Arc};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{StreamTurn, builder::parse_tool_call, error_flow, turn_flow};
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
            self.remove_session(session).await;
        } else {
            self.disconnect_stream(session, Arc::clone(events)).await;
        }
    }

    pub(in crate::anthropic) async fn disconnect_stream(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
    ) -> StreamTurn {
        // Cancel before unregistering so a racing outer follow-up can still
        // discover this session, preempt the gate, and reuse the provider thread.
        match self.app.cancel_turn(&session.thread_id).await {
            Ok(TurnCancellation::Settled) => {
                self.remove_session(session).await;
            }
            Ok(TurnCancellation::Unsupported) => {
                self.handle_unsupported_disconnect(session, events).await;
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
    ) {
        if session.pending_tools.lock().await.is_empty() {
            // No tool call has reached Claude Code yet. Keep consuming the
            // non-cancellable turn so a delayed call receives a rejection.
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
