use std::{ops::ControlFlow, sync::Arc};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{StreamTurn, builder::parse_tool_call, error_flow, turn_flow};
use crate::{
    agent_backend::TurnCancellation,
    anthropic::{Bridge, Session},
};

impl Bridge {
    pub(super) async fn finish_closed_stream(
        &self,
        session: &Arc<Session>,
        events: &crate::app_server::ThreadEvents,
        provider_settled: bool,
    ) {
        if provider_settled {
            self.remove_session(session).await;
        } else {
            self.disconnect_stream(session, events).await;
        }
    }

    pub(super) async fn disconnect_stream(
        &self,
        session: &Arc<Session>,
        events: &crate::app_server::ThreadEvents,
    ) -> StreamTurn {
        self.remove_session(session).await;
        match self.app.cancel_turn(&session.thread_id).await {
            Ok(TurnCancellation::Settled) => {}
            Ok(TurnCancellation::Unsupported) => {
                if let Err(error) = self.drain_disconnected_turn(session, events).await {
                    tracing::warn!(
                        %error,
                        thread_id = %session.thread_id,
                        "failed to drain disconnected non-cancellable turn"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    thread_id = %session.thread_id,
                    "failed to cancel disconnected streaming turn"
                );
            }
        }
        StreamTurn::Disconnected
    }

    async fn drain_disconnected_turn(
        &self,
        session: &Session,
        events: &crate::app_server::ThreadEvents,
    ) -> Result<()> {
        self.reject_pending_disconnected_tools(session).await?;
        loop {
            let event = events
                .recv()
                .await
                .context("app-server event stream closed while draining disconnected turn")?;
            match event.get("method").and_then(Value::as_str) {
                Some("item/tool/call") => {
                    let request_id = parse_tool_call(&event)?.request_id;
                    self.reject_disconnected_tool(session, request_id).await?;
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

    async fn reject_pending_disconnected_tools(&self, session: &Session) -> Result<()> {
        loop {
            let pending = session
                .pending_tools
                .lock()
                .await
                .iter()
                .next()
                .map(|(tool_use_id, request_id)| (tool_use_id.clone(), request_id.clone()));
            let Some((tool_use_id, request_id)) = pending else {
                break;
            };
            self.reject_disconnected_tool(session, request_id).await?;
            session.pending_tools.lock().await.remove(&tool_use_id);
            self.agent_efforts
                .remove_tool_results(std::iter::once(tool_use_id.as_str()));
        }
        *session
            .pending_since
            .lock()
            .expect("pending tool clock poisoned") = None;
        Ok(())
    }

    async fn reject_disconnected_tool(&self, session: &Session, request_id: Value) -> Result<()> {
        self.app
            .respond_for_model(
                &session.model,
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
}
