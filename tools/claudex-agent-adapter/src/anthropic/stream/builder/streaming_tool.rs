use anyhow::Result;
use serde_json::{Value, json};
use uuid::Uuid;

use super::external_tool::ExternalToolContext;
use super::{SegmentBuilder, StreamingToolUse, external_tool, parse_tool_delta, parse_tool_start};
use crate::anthropic::Session;
use crate::anthropic::retention::record_pending_tool;
use crate::anthropic::stream::protocol::{
    StreamSender, send_content_block_stop, send_input_json_delta, send_tool_use_start,
};

impl SegmentBuilder {
    pub(super) async fn start_native_tool_use(
        &mut self,
        session: &Session,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let start = parse_tool_start(event)?;
        let Some(original_name) =
            external_tool::requested_external_tool_name(&session.external_tool_names, &start.name)
        else {
            return Ok(());
        };
        self.start_executable_tool_use_card(&start.call_id, original_name, stream)
            .await
    }

    pub(in crate::anthropic::stream) async fn start_executable_tool_use_card(
        &mut self,
        call_id: &str,
        original_name: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if crate::anthropic::agent_effort::is_agent_tool(original_name)
            || self.streaming_tool.is_some()
        {
            return Ok(());
        }
        self.note_provider_turn_activity();
        self.prepare_blocks_for_external_tool(original_name, call_id, stream)
            .await?;
        let tool_use_id = format!("toolu_{}", Uuid::new_v4().simple());
        let index = self.blocks.len();
        self.blocks.push(json!({
            "type":"tool_use",
            "id":tool_use_id,
            "name":original_name,
            "input":{}
        }));
        send_tool_use_start(stream, index, &tool_use_id, original_name).await?;
        self.external_tool_calls += 1;
        self.streaming_tool = Some(StreamingToolUse {
            call_id: call_id.to_owned(),
            tool_use_id,
            index,
            partial_json: String::new(),
        });
        Ok(())
    }

    pub(super) async fn delta_native_tool_use(
        &mut self,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let delta = parse_tool_delta(event)?;
        self.append_native_tool_use_delta(&delta.call_id, &delta.delta, stream)
            .await
    }

    pub(in crate::anthropic::stream) async fn append_native_tool_use_delta(
        &mut self,
        call_id: &str,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some(index) = self.push_streaming_delta(call_id, delta) else {
            return Ok(());
        };
        self.saw_provider_turn_activity = true;
        self.note_visible_provider_activity();
        send_input_json_delta(stream, index, delta).await
    }

    fn push_streaming_delta(&mut self, call_id: &str, delta: &str) -> Option<usize> {
        let open = self.streaming_tool.as_mut()?;
        if open.call_id != call_id || delta.is_empty() {
            return None;
        }
        open.partial_json.push_str(delta);
        Some(open.index)
    }

    pub(super) fn take_streaming_tool(&mut self, call_id: &str) -> Option<StreamingToolUse> {
        let open = self.streaming_tool.as_ref()?;
        if open.call_id != call_id {
            return None;
        }
        self.streaming_tool.take()
    }

    pub(super) async fn finish_native_tool_use(
        &mut self,
        context: ExternalToolContext<'_>,
        original_name: &str,
        open: StreamingToolUse,
        request_id: Value,
        arguments: Value,
    ) -> Result<()> {
        let (intent_arguments, claude_arguments) =
            crate::anthropic::agent_effort::prepare_arguments_for_user(
                original_name,
                &open.tool_use_id,
                &arguments,
                context.current_messages,
                context.system,
            );
        if let Some(arguments) = intent_arguments.as_ref() {
            context.bridge.agent_efforts.record_from_user_messages(
                crate::anthropic::agent_effort::AgentEffortRecord {
                    client_user_id: context.session.client_user_id.as_deref(),
                    tool_name: original_name,
                    tool_use_id: open.tool_use_id.clone(),
                    parent_model: &context.session.model,
                    arguments,
                    user_messages: context.current_messages,
                    system: context.system,
                },
                Some(context.bridge.model_catalog()),
            );
        }
        record_pending_tool(
            context.session,
            open.tool_use_id.clone(),
            request_id,
            std::time::Instant::now(),
        )
        .await;
        self.report_subagent_action(original_name, &arguments, context.stream)
            .await?;
        if open.partial_json.is_empty() {
            send_input_json_delta(context.stream, open.index, &claude_arguments.to_string())
                .await?;
        }
        send_content_block_stop(context.stream, open.index).await?;
        self.blocks[open.index] = json!({
            "type":"tool_use",
            "id":open.tool_use_id,
            "name":original_name,
            "input":claude_arguments
        });
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "streaming_tool_tests.rs"]
mod tests;
