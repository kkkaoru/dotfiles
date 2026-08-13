use anyhow::Result;
use serde_json::Value;

use super::SegmentBuilder;
use crate::anthropic::stream::acp_tool_bridge;
use crate::anthropic::{Bridge, Session};

use super::super::protocol::StreamSender;

struct LaunchBridge<'a> {
    bridge: &'a Bridge,
    session: &'a Session,
    current_messages: &'a [Value],
    system: &'a Value,
    stream: Option<&'a StreamSender>,
}

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
        let params = event.get("params");
        let tool = params
            .and_then(|params| params.get("tool"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let title = params
            .and_then(|params| params.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let mcp_hint =
            call_id.is_some_and(|id| self.mcp_provider_call_ids.iter().any(|known| known == id));
        let mcp_shaped = mcp_hint
            || tool.eq_ignore_ascii_case("mcp")
            || title.to_ascii_lowercase().starts_with("mcp");
        let ctx = LaunchBridge {
            bridge,
            session,
            current_messages,
            system,
            stream,
        };
        if self
            .try_bridge_provider_launch(&ctx, event, mcp_shaped)
            .await?
        {
            return Ok(());
        }
        self.note_mcp_provider_call(call_id, mcp_shaped);
        if self.should_suppress_unbridged_launch(session, event, mcp_hint, call_id) {
            return Ok(());
        }
        if method == Some("item/providerTool/call") {
            self.provider_tool_call(event, stream).await
        } else {
            self.provider_tool_update(event, stream).await
        }
    }

    async fn try_bridge_provider_launch(
        &mut self,
        ctx: &LaunchBridge<'_>,
        event: &Value,
        mcp_shaped: bool,
    ) -> Result<bool> {
        // Always pass the Claude session owner for MCP-shaped cards so the first
        // empty card can consume `launch-queue.<session>.jsonl` (not only updates).
        let bridged = if mcp_shaped {
            acp_tool_bridge::bridge_provider_tool_call_with_mcp_hint(
                &ctx.session.external_tool_names,
                event,
                ctx.session.claude_session_id.as_deref(),
            )
        } else {
            acp_tool_bridge::bridge_provider_tool_call(&ctx.session.external_tool_names, event)
        };
        let Some(call) = bridged else {
            return Ok(false);
        };
        let bridged_id = call.call_id.clone();
        self.record_bridged_provider_launch(ctx, call).await?;
        self.incomplete_launch_call_ids
            .retain(|id| id != &bridged_id);
        if mcp_shaped {
            self.drain_queued_launches(ctx).await?;
        }
        Ok(true)
    }

    fn note_mcp_provider_call(&mut self, call_id: Option<&str>, mcp_shaped: bool) {
        let Some(call_id) = call_id else {
            return;
        };
        if mcp_shaped && !self.mcp_provider_call_ids.iter().any(|id| id == call_id) {
            self.mcp_provider_call_ids.push(call_id.to_owned());
        }
    }

    fn should_suppress_unbridged_launch(
        &mut self,
        session: &Session,
        event: &Value,
        mcp_hint: bool,
        call_id: Option<&str>,
    ) -> bool {
        let suppress = mcp_hint
            || acp_tool_bridge::is_unbridged_launch_progress(&session.external_tool_names, event);
        if !suppress {
            return false;
        }
        if let Some(call_id) = call_id {
            self.track_incomplete_launch(call_id);
        }
        true
    }

    fn track_incomplete_launch(&mut self, call_id: &str) {
        let already_known = self
            .incomplete_launch_call_ids
            .iter()
            .any(|id| id == call_id)
            || self
                .bridged_provider_launch_ids
                .iter()
                .any(|id| id == call_id);
        if !already_known {
            self.incomplete_launch_call_ids.push(call_id.to_owned());
        }
    }

    pub(super) async fn drain_remaining_queued_launches(
        &mut self,
        bridge: &Bridge,
        session: &Session,
        current_messages: &[Value],
        system: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.drain_queued_launches(&LaunchBridge {
            bridge,
            session,
            current_messages,
            system,
            stream,
        })
        .await
    }

    async fn drain_queued_launches(&mut self, ctx: &LaunchBridge<'_>) -> Result<()> {
        let owner = ctx.session.claude_session_id.as_deref();
        let mut drained = 0usize;
        while let Some(arguments) =
            super::super::acp_launch_queue::take_pending_launch_arguments_for(owner)
        {
            drained += 1;
            self.bridge_one_queued_launch(ctx, owner, drained, arguments)
                .await?;
        }
        if drained > 0 {
            tracing::info!(
                drained,
                launch_owner = owner,
                "bridged remaining queued claudex-launch MCP entries in the same turn"
            );
        }
        Ok(())
    }

    async fn bridge_one_queued_launch(
        &mut self,
        ctx: &LaunchBridge<'_>,
        owner: Option<&str>,
        drained: usize,
        arguments: Value,
    ) -> Result<()> {
        let Some(call) = self.queued_launch_tool_call(ctx, owner, drained, arguments) else {
            return Ok(());
        };
        self.record_bridged_provider_launch(ctx, call).await
    }

    fn queued_launch_tool_call(
        &self,
        ctx: &LaunchBridge<'_>,
        owner: Option<&str>,
        drained: usize,
        arguments: Value,
    ) -> Option<crate::anthropic::stream::ToolCall> {
        let call_id = format!(
            "queue-drain-{}-{drained}",
            owner.unwrap_or("global").replace(['/', ' '], "_")
        );
        let call = acp_tool_bridge::tool_call_from_launch_queue_arguments(
            &ctx.session.external_tool_names,
            &call_id,
            arguments,
        );
        if call.is_none() {
            tracing::warn!(
                %call_id,
                launch_owner = owner,
                "skipped queued claudex-launch entry without a bridgeable prompt"
            );
        }
        call
    }

    pub(super) async fn report_incomplete_launches(
        &mut self,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let pending = self
            .incomplete_launch_call_ids
            .iter()
            .filter(|id| {
                !self
                    .bridged_provider_launch_ids
                    .iter()
                    .any(|bridged| bridged == *id)
            })
            .cloned()
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return Ok(());
        }
        let message = format!(
            "Claudex: {} SubAgent launch card(s) never received a prompt and were not started (callIds: {}). Re-issue Agent/Task with a non-empty prompt in the same turn; `_toolName`-only cards do not start workers.",
            pending.len(),
            pending.join(", ")
        );
        tracing::warn!(
            incomplete = pending.len(),
            call_ids = %pending.join(","),
            "incomplete ACP launch cards dropped without a prompt"
        );
        self.stream_progress_text(&message, stream).await?;
        self.incomplete_launch_call_ids.clear();
        Ok(())
    }

    async fn record_bridged_provider_launch(
        &mut self,
        ctx: &LaunchBridge<'_>,
        call: crate::anthropic::stream::ToolCall,
    ) -> Result<()> {
        if self
            .bridged_provider_launch_ids
            .iter()
            .any(|id| id == &call.call_id)
        {
            return Ok(());
        }
        self.bridged_provider_launch_ids.push(call.call_id.clone());
        tracing::info!(
            call_id = %call.call_id,
            name = %call.name,
            "bridging ACP providerTool launch to Claude Code tool_use"
        );
        self.tool_call(
            ctx.bridge,
            ctx.session,
            ctx.current_messages,
            ctx.system,
            call,
            ctx.stream,
        )
        .await
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "provider_launch_edge_tests.rs"]
mod edge_tests;
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "provider_launch_tests.rs"]
mod tests;
