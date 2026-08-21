use anyhow::{Result, bail};
use serde_json::{Value, json};
use uuid::Uuid;

use super::external_tool::ExternalToolContext;
use super::{SegmentBuilder, StreamingToolUse, external_tool, parse_tool_delta, parse_tool_start};
use crate::anthropic::Session;
use crate::anthropic::retention::record_pending_tool;
use crate::anthropic::stream::protocol::{
    StreamSender, send_content_block_stop, send_input_json_delta, send_tool_use_start,
};

#[path = "streaming_tool_ready.rs"]
mod streaming_tool_ready;
use streaming_tool_ready::{
    ToolJsonReadiness, finished_tool_json_payload, incomplete_tool_json_error,
    requires_complete_tool_json, tool_input_ready, tool_json_readiness,
};

const INVALID_TOOL_JSON_CIRCUIT_LIMIT: u8 = 3;

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
        if self.consecutive_invalid_tool_json >= INVALID_TOOL_JSON_CIRCUIT_LIMIT {
            bail!("{}", invalid_tool_json_circuit_error());
        }
        if crate::anthropic::agent_effort::is_agent_tool(original_name)
            || self.streaming_tool.is_some()
        {
            return Ok(());
        }
        self.note_provider_turn_activity();
        if requires_complete_tool_json(original_name) {
            self.streaming_tool = Some(unstarted_streaming_tool(call_id, original_name));
            return Ok(());
        }
        self.open_streaming_tool_on_wire(call_id, original_name, stream)
            .await
    }

    async fn open_streaming_tool_on_wire(
        &mut self,
        call_id: &str,
        original_name: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let mut open = unstarted_streaming_tool(call_id, original_name);
        self.start_streaming_tool_sse(&mut open, original_name, stream)
            .await?;
        self.streaming_tool = Some(open);
        Ok(())
    }

    async fn start_streaming_tool_sse(
        &mut self,
        open: &mut StreamingToolUse,
        name: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if open.sse_started {
            return Ok(());
        }
        self.prepare_blocks_for_external_tool(name, &open.call_id, stream)
            .await?;
        let index = self.blocks.len();
        let tool_use_id = open.tool_use_id.clone();
        self.blocks.push(json!({
            "type":"tool_use",
            "id":tool_use_id,
            "name":name,
            "input":{}
        }));
        send_tool_use_start(stream, index, &tool_use_id, name).await?;
        self.external_tool_calls += 1;
        open.index = index;
        open.sse_started = true;
        Ok(())
    }

    async fn start_held_streaming_tool_sse(&mut self, stream: Option<&StreamSender>) -> Result<()> {
        let Some(mut open) = self.streaming_tool.take() else {
            return Ok(());
        };
        let name = open.name.clone();
        let started = self
            .start_streaming_tool_sse(&mut open, &name, stream)
            .await;
        self.streaming_tool = Some(open);
        started
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
        if self.push_streaming_delta(call_id, delta).is_none() {
            return Ok(());
        }
        self.saw_provider_turn_activity = true;
        self.note_visible_provider_activity();
        self.emit_complete_streaming_json(stream).await
    }

    async fn emit_complete_streaming_json(&mut self, stream: Option<&StreamSender>) -> Result<()> {
        match self.pending_tool_json_readiness() {
            None => Ok(()),
            Some(ToolJsonReadiness::Truncated) => Ok(()),
            Some(ToolJsonReadiness::Incomplete) => self.record_invalid_tool_json(),
            Some(ToolJsonReadiness::Ready) => self.emit_ready_streaming_json(stream).await,
        }
    }

    fn pending_tool_json_readiness(&self) -> Option<ToolJsonReadiness> {
        let open = self.streaming_tool.as_ref()?;
        if open.json_emitted {
            return None;
        }
        Some(tool_json_readiness(&open.name, &open.partial_json))
    }

    async fn emit_ready_streaming_json(&mut self, stream: Option<&StreamSender>) -> Result<()> {
        self.start_held_streaming_tool_sse(stream).await?;
        let Some((index, payload)) = self.take_ready_streaming_json() else {
            return Ok(());
        };
        self.consecutive_invalid_tool_json = 0;
        send_input_json_delta(stream, index, &payload).await
    }

    fn take_ready_streaming_json(&mut self) -> Option<(usize, String)> {
        let open = self.streaming_tool.as_mut()?;
        open.json_emitted = true;
        Some((open.index, open.partial_json.clone()))
    }

    fn record_invalid_tool_json(&mut self) -> Result<()> {
        let next = self.consecutive_invalid_tool_json.saturating_add(1);
        self.consecutive_invalid_tool_json = next;
        if next >= INVALID_TOOL_JSON_CIRCUIT_LIMIT {
            bail!("{}", invalid_tool_json_circuit_error());
        }
        Ok(())
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

    pub(super) async fn stop_or_reject_open_streaming_tool(
        &mut self,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some(open) = self.streaming_tool.take() else {
            return Ok(());
        };
        if !open.json_emitted
            && requires_complete_tool_json(&open.name)
            && !tool_input_ready(&open.name, &open.partial_json, &Value::Null)
        {
            self.record_invalid_tool_json()?;
            bail!("{}", incomplete_tool_json_error(&open.name));
        }
        if !open.sse_started {
            return Ok(());
        }
        send_content_block_stop(stream, open.index).await
    }

    pub(super) async fn finish_native_tool_use(
        &mut self,
        context: ExternalToolContext<'_>,
        original_name: &str,
        open: StreamingToolUse,
        request_id: Value,
        arguments: Value,
    ) -> Result<()> {
        self.reject_incomplete_finished_tool(original_name, &open, &arguments)?;
        let mut open = open;
        self.start_streaming_tool_sse(&mut open, original_name, context.stream)
            .await?;
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
        if !open.json_emitted {
            let payload =
                finished_tool_json_payload(original_name, &open.partial_json, &claude_arguments);
            send_input_json_delta(context.stream, open.index, &payload).await?;
            self.consecutive_invalid_tool_json = 0;
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

    fn reject_incomplete_finished_tool(
        &mut self,
        name: &str,
        open: &StreamingToolUse,
        arguments: &Value,
    ) -> Result<()> {
        if open.json_emitted || tool_input_ready(name, &open.partial_json, arguments) {
            return Ok(());
        }
        self.record_invalid_tool_json()?;
        bail!("{}", incomplete_tool_json_error(name));
    }
}

fn unstarted_streaming_tool(call_id: &str, name: &str) -> StreamingToolUse {
    StreamingToolUse {
        call_id: call_id.to_owned(),
        tool_use_id: format!("toolu_{}", Uuid::new_v4().simple()),
        index: 0,
        name: name.to_owned(),
        partial_json: String::new(),
        json_emitted: false,
        sse_started: false,
    }
}

fn invalid_tool_json_circuit_error() -> String {
    format!(
        "Stopped emitting tool_use after {INVALID_TOOL_JSON_CIRCUIT_LIMIT} consecutive empty or invalid JSON payloads."
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "streaming_tool_tests.rs"]
mod tests;
