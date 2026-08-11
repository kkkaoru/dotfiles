use std::{collections::HashSet, ops::ControlFlow, sync::Arc};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::super::{builder::parse_tool_call, error_flow, turn_flow};
use super::warn_disconnect_failure;
use crate::anthropic::content::pending_request_id;
use crate::anthropic::{Bridge, Session};

pub(super) async fn drain_disconnected_turn_with_warning(
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

pub(super) async fn reject_disconnected_tool_with_warning(
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

pub(in crate::anthropic::stream) async fn drain_disconnected_turn(
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

pub(super) async fn take_pending_disconnected_tools(session: &Session) -> Vec<(String, Value)> {
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

pub(super) fn request_id_keys(tools: &[(String, Value)]) -> HashSet<String> {
    tools
        .iter()
        .map(|(_, request_id)| {
            serde_json::to_string(request_id).expect("tool request id is serializable")
        })
        .collect()
}

pub(super) async fn reject_disconnected_tool_once(
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
