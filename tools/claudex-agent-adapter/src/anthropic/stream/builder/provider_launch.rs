use anyhow::Result;
use serde_json::Value;

use super::SegmentBuilder;
use crate::anthropic::{Bridge, Session};

use super::super::protocol::StreamSender;

impl SegmentBuilder {
    pub(super) async fn provider_launch_event(
        &mut self,
        bridge: &Bridge,
        session: &Session,
        current_messages: &[Value],
        system: &Value,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let method = event.get("method").and_then(Value::as_str);
        let call_id = event
            .get("params")
            .and_then(|params| params.get("callId"))
            .and_then(Value::as_str);
        let mcp_hint =
            call_id.is_some_and(|id| self.mcp_provider_call_ids.iter().any(|known| known == id));
        let bridged = if mcp_hint {
            super::super::acp_tool_bridge::bridge_provider_tool_call_with_mcp_hint(
                &session.external_tool_names,
                event,
            )
        } else {
            super::super::acp_tool_bridge::bridge_provider_tool_call(
                &session.external_tool_names,
                event,
            )
        };
        if let Some(call) = bridged {
            if !self
                .bridged_provider_launch_ids
                .iter()
                .any(|id| id == &call.call_id)
            {
                self.bridged_provider_launch_ids
                    .push(call.call_id.clone());
                tracing::info!(
                    call_id = %call.call_id,
                    name = %call.name,
                    "bridging ACP providerTool launch to Claude Code tool_use"
                );
                self.tool_call(bridge, session, current_messages, system, call, stream)
                    .await?;
            }
            return Ok(());
        }
        let params = event.get("params");
        let tool = params
            .and_then(|params| params.get("tool"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let title = params
            .and_then(|params| params.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(call_id) = call_id
            && (tool.eq_ignore_ascii_case("mcp") || title.to_ascii_lowercase().starts_with("mcp"))
            && !self.mcp_provider_call_ids.iter().any(|id| id == call_id)
        {
            self.mcp_provider_call_ids.push(call_id.to_owned());
        }
        let suppress = mcp_hint
            || super::super::acp_tool_bridge::is_unbridged_launch_progress(
                &session.external_tool_names,
                event,
            );
        if suppress {
            return Ok(());
        }
        if method == Some("item/providerTool/call") {
            self.provider_tool_call(event, stream).await
        } else {
            self.provider_tool_update(event, stream).await
        }
    }
}
