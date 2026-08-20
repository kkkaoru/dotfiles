use anyhow::{Result, bail};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::external_tool::ExternalToolContext;
use super::{SegmentBuilder, StreamingToolUse, external_tool, parse_tool_delta, parse_tool_start};
use crate::anthropic::Session;
use crate::anthropic::retention::record_pending_tool;
use crate::anthropic::stream::protocol::{
    StreamSender, send_content_block_stop, send_input_json_delta, send_tool_use_start,
};
const INVALID_TOOL_JSON_CIRCUIT_LIMIT: u8 = 3;
const BASH_COMMAND_KEYS: [&str; 4] = ["command", "cmd", "script", "bash"];
const SEND_MESSAGE_RECIPIENT_KEYS: [&str; 3] = ["to", "resume_from", "resume"];
const SEND_MESSAGE_BODY_KEYS: [&str; 6] =
    ["prompt", "message", "task", "instruction", "query", "input"];
const INCOMPLETE_BASH_JSON: &str =
    "Incomplete Bash tool JSON was not flushed; a non-empty command is required.";
const INCOMPLETE_SEND_MESSAGE_JSON: &str =
    "Incomplete SendMessage tool JSON was not flushed; non-empty to and message are required.";
const INCOMPLETE_TOOL_JSON: &str =
    "Incomplete tool JSON was not flushed; required keys are missing.";

enum ToolJsonReadiness {
    Truncated,
    Incomplete,
    Ready,
}
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
            let payload = finished_tool_json_payload(original_name, &open, &claude_arguments);
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

fn tool_json_readiness(name: &str, raw: &str) -> ToolJsonReadiness {
    if raw.trim().is_empty() {
        return ToolJsonReadiness::Incomplete;
    }
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return ToolJsonReadiness::Truncated;
    };
    match tool_arguments_ready(name, &value) {
        true => ToolJsonReadiness::Ready,
        false => ToolJsonReadiness::Incomplete,
    }
}

fn tool_input_ready(name: &str, partial_json: &str, arguments: &Value) -> bool {
    tool_arguments_ready(name, arguments)
        || matches!(
            tool_json_readiness(name, partial_json),
            ToolJsonReadiness::Ready
        )
}

fn tool_arguments_ready(name: &str, arguments: &Value) -> bool {
    let Some(object) = arguments.as_object() else {
        return false;
    };
    if name.eq_ignore_ascii_case("Bash") {
        return bash_command_present(object);
    }
    if name.eq_ignore_ascii_case("SendMessage") {
        return send_message_ready(object);
    }
    true
}

fn bash_command_present(object: &Map<String, Value>) -> bool {
    BASH_COMMAND_KEYS.iter().any(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|command| !command.is_empty())
    })
}

fn send_message_ready(object: &Map<String, Value>) -> bool {
    first_nonempty(object, &SEND_MESSAGE_RECIPIENT_KEYS).is_some()
        && first_nonempty(object, &SEND_MESSAGE_BODY_KEYS).is_some()
}

fn first_nonempty<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

fn requires_complete_tool_json(name: &str) -> bool {
    name.eq_ignore_ascii_case("Bash") || name.eq_ignore_ascii_case("SendMessage")
}

fn incomplete_tool_json_error(name: &str) -> &'static str {
    if name.eq_ignore_ascii_case("Bash") {
        INCOMPLETE_BASH_JSON
    } else if name.eq_ignore_ascii_case("SendMessage") {
        INCOMPLETE_SEND_MESSAGE_JSON
    } else {
        INCOMPLETE_TOOL_JSON
    }
}

fn invalid_tool_json_circuit_error() -> String {
    format!(
        "Stopped emitting tool_use after {INVALID_TOOL_JSON_CIRCUIT_LIMIT} consecutive empty or invalid JSON payloads."
    )
}

fn finished_tool_json_payload(
    name: &str,
    open: &StreamingToolUse,
    claude_arguments: &Value,
) -> String {
    match tool_arguments_ready(name, claude_arguments) {
        true => claude_arguments.to_string(),
        false => open.partial_json.clone(),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "streaming_tool_tests.rs"]
mod tests;
