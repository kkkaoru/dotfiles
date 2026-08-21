use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::events_circuit::{should_emit_tool_start, should_forward_finished_tool};
use super::events_finish::{
    current_tool_arguments, event_index, finished_tool_arguments, mapped_claude_code_tool,
    mapped_start_tool_name, tool_id, tool_name,
};
use super::{EventTranslateState, PiGateway, ToolCallBuffer};
use crate::anthropic::token_efficiency::tool_arguments_are_unusable;

const TOOL_START_METHOD: &str = "item/tool/start";
const TOOL_DELTA_METHOD: &str = "item/tool/delta";

fn start_card_tool_name<'a>(provider_name: &'a str, mapped_name: &'a str) -> &'a str {
    if mapped_name == "SendMessage" {
        mapped_name
    } else {
        mapped_start_tool_name(provider_name)
    }
}

fn skip_start_until_ready(name: &str, mapped: &Value, raw_arguments: &str) -> bool {
    if !tool_arguments_are_unusable(name, mapped) {
        return false;
    }
    if name.eq_ignore_ascii_case("Bash") {
        return raw_arguments.is_empty() || serde_json::from_str::<Value>(raw_arguments).is_ok();
    }
    name.eq_ignore_ascii_case("SendMessage")
        || name.eq_ignore_ascii_case("Agent")
        || name.eq_ignore_ascii_case("Task")
        || name.to_ascii_lowercase().contains("spawn_subagent")
}

impl PiGateway {
    pub(super) fn emit_tool_start_if_ready(
        &self,
        thread_id: &str,
        request_id: &str,
        event: &Value,
        state: &mut EventTranslateState,
    ) -> Result<()> {
        let circuit_open = !should_emit_tool_start(state);
        let index = event_index(event)?;
        let Some(tool) = state.tools.get_mut(&index) else {
            return Ok(());
        };
        if let Some(id) = tool_id(event).filter(|id| !id.is_empty()) {
            tool.id = id.to_owned();
        }
        if let Some(name) = tool_name(event).filter(|name| !name.is_empty()) {
            tool.name = name.to_owned();
        }
        if circuit_open || tool.start_emitted || tool.id.is_empty() || tool.name.is_empty() {
            return Ok(());
        }
        let arguments = current_tool_arguments(event, tool);
        let Some((mapped_name, mapped)) =
            mapped_claude_code_tool(&tool.name, arguments, &state.listed_tools)
        else {
            return Ok(());
        };
        if skip_start_until_ready(&mapped_name, &mapped, &tool.arguments) {
            return Ok(());
        }
        let start_name = start_card_tool_name(&tool.name, &mapped_name);
        tool.start_emitted = true;
        self.events.dispatch_to(
            request_id,
            json!({
                "id":tool.id,
                "method":TOOL_START_METHOD,
                "params":{
                    "threadId":thread_id,
                    "callId":tool.id,
                    "tool":start_name
                }
            }),
        );
        Ok(())
    }

    pub(super) fn emit_tool_delta(
        &self,
        thread_id: &str,
        request_id: &str,
        event: &Value,
        tools: &HashMap<u64, ToolCallBuffer>,
    ) -> Result<()> {
        let index = event_index(event)?;
        let Some(tool) = tools.get(&index) else {
            return Ok(());
        };
        if !tool.start_emitted {
            return Ok(());
        }
        let Some(delta) = event
            .get("delta")
            .and_then(Value::as_str)
            .filter(|delta| !delta.is_empty())
        else {
            return Ok(());
        };
        self.events.dispatch_to(
            request_id,
            json!({
                "id":tool.id,
                "method":TOOL_DELTA_METHOD,
                "params":{
                    "threadId":thread_id,
                    "callId":tool.id,
                    "delta":delta
                }
            }),
        );
        Ok(())
    }

    pub(super) fn finish_tool_call(
        &self,
        thread_id: &str,
        request_id: &str,
        event: &Value,
        state: &mut EventTranslateState,
    ) -> Result<()> {
        let index = event_index(event)?;
        let mut tool = state
            .tools
            .remove(&index)
            .context("Pi toolcall_end did not match toolcall_start")?;
        if let Some(id) = tool_id(event) {
            tool.id = id.to_owned();
        }
        if let Some(name) = tool_name(event) {
            tool.name = name.to_owned();
        }
        let arguments = finished_tool_arguments(event, &tool)?;
        if tool.id.is_empty() || tool.name.is_empty() {
            bail!("Pi tool call omitted id or name");
        }
        let Some((name, arguments)) =
            mapped_claude_code_tool(&tool.name, arguments, &state.listed_tools)
        else {
            return Ok(());
        };
        if !should_forward_finished_tool(state, &name, &arguments) {
            return Ok(());
        }
        if !tool.start_emitted {
            self.events.dispatch_to(
                request_id,
                json!({
                    "id":tool.id,
                    "method":TOOL_START_METHOD,
                    "params":{
                        "threadId":thread_id,
                        "callId":tool.id,
                        "tool":start_card_tool_name(&tool.name, &name)
                    }
                }),
            );
        }
        self.events.dispatch_to(
            request_id,
            json!({
                "id":tool.id,
                "method":"item/tool/call",
                "params":{
                    "threadId":thread_id,
                    "callId":tool.id,
                    "tool":name,
                    "arguments":arguments
                }
            }),
        );
        Ok(())
    }
}
