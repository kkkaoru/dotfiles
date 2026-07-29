use anyhow::Result;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{SegmentBuilder, ToolCall};
use crate::anthropic::stream::protocol::{StreamSender, send_tool_use};
use crate::anthropic::{Bridge, Session, retention::record_pending_tool};

#[derive(Clone, Copy)]
pub(super) struct ExternalToolContext<'a> {
    pub(super) bridge: &'a Bridge,
    pub(super) session: &'a Session,
    pub(super) current_messages: &'a [Value],
    pub(super) system: &'a Value,
    pub(super) stream: Option<&'a StreamSender>,
}

impl SegmentBuilder {
    pub(super) async fn external_tool_call(
        &mut self,
        context: ExternalToolContext<'_>,
        original_name: &str,
        call: ToolCall<'_>,
    ) -> Result<()> {
        let mut arguments = call.arguments.clone();
        crate::anthropic::agent_routing::hydrate_routing_fields_from_context(
            &mut arguments,
            context.current_messages,
            context.system,
            context.bridge.model_catalog(),
        );
        crate::anthropic::agent_routing::hydrate_standard_agent_to_parent(
            &mut arguments,
            &context.session.model,
        );
        crate::anthropic::agent_effort::validate_routed_agent_arguments_with_catalog(
            original_name,
            &arguments,
            context.current_messages,
            context.system,
            context.bridge.model_catalog(),
        )?;
        let tool_use_id = format!("toolu_{}", Uuid::new_v4().simple());
        let (intent_arguments, claude_arguments) =
            crate::anthropic::agent_effort::prepare_arguments_for_user(
                original_name,
                &tool_use_id,
                &arguments,
                context.current_messages,
                context.system,
            );
        if let Some(arguments) = intent_arguments.as_ref() {
            context.bridge.agent_efforts.record_from_user_messages(
                crate::anthropic::agent_effort::AgentEffortRecord {
                    client_user_id: context.session.client_user_id.as_deref(),
                    tool_name: original_name,
                    tool_use_id: tool_use_id.clone(),
                    parent_model: &context.session.model,
                    arguments,
                    user_messages: context.current_messages,
                    system: context.system,
                },
                Some(context.bridge.model_catalog()),
            );
        }
        tracing::debug!(call_id = %call.call_id, %tool_use_id, "mapped app-server tool call");
        record_pending_tool(
            context.session,
            tool_use_id.clone(),
            call.request_id,
            std::time::Instant::now(),
        )
        .await;
        self.close_open_blocks(context.stream).await?;
        let block = json!({
            "type": "tool_use",
            "id": tool_use_id,
            "name": original_name,
            "input": claude_arguments
        });
        let index = self.blocks.len();
        send_tool_use(context.stream, index, &block).await?;
        self.blocks.push(block);
        self.external_tool_calls += 1;
        Ok(())
    }
}
