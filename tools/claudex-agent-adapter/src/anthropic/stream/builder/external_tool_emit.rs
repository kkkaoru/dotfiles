use anyhow::Result;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{ExternalToolContext, SegmentBuilder};
use crate::anthropic::retention::record_pending_tool;
use crate::anthropic::stream::protocol::{StreamSender, send_tool_use};

impl SegmentBuilder {
    pub(super) async fn emit_external_tool_use(
        &mut self,
        context: ExternalToolContext<'_>,
        original_name: &str,
        call_id: String,
        request_id: Value,
        arguments: Value,
    ) -> Result<()> {
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
        tracing::debug!(%call_id, %tool_use_id, "mapped app-server tool call");
        record_pending_tool(
            context.session,
            tool_use_id.clone(),
            request_id,
            std::time::Instant::now(),
        )
        .await;
        self.report_subagent_action(original_name, &arguments, context.stream)
            .await?;
        self.prepare_blocks_for_external_tool(original_name, &call_id, context.stream)
            .await?;
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

    pub(super) async fn prepare_blocks_for_external_tool(
        &mut self,
        original_name: &str,
        call_id: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let _ = (original_name, call_id);
        if !self.is_subagent {
            return self.close_open_blocks(stream).await;
        }
        // Replace live provider chrome with clean buffered CoT before closing.
        // Waiting until finish can no longer reconcile A + tool chrome + B
        // with the A+B buffer and would synthesize an unstarted SSE block.
        self.close_text_block(stream).await?;
        self.commit_pending_reasoning_for_transcript(None).await?;
        self.thinking
            .close_before_executable_tool_use(&mut self.blocks, stream)
            .await
    }
}
