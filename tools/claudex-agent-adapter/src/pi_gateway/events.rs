use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};
use serde_json::{Value, json};

use super::{PiGateway, protocol};

#[path = "events_circuit.rs"]
mod events_circuit;
#[path = "events_dispatch.rs"]
mod events_dispatch;
#[path = "events_finish.rs"]
mod events_finish;
#[path = "events_skill.rs"]
mod events_skill;
#[path = "events_tools.rs"]
mod events_tools;
use events_finish::{anthropic_stop_reason, append_tool_call, mark_streamed, start_tool_call};

#[derive(Default)]
pub(super) struct ToolCallBuffer {
    id: String,
    name: String,
    arguments: String,
    start_emitted: bool,
}

#[derive(Default)]
pub(super) struct EventTranslateState {
    tools: HashMap<u64, ToolCallBuffer>,
    streamed_content: HashSet<u64>,
    completed_thinking: HashSet<u64>,
    listed_tools: HashSet<String>,
    skill_recovery_arguments: Option<Value>,
    consecutive_unusable_tools: u8,
    forwarded_tool_calls: usize,
}
pub(super) use events_finish::event_translate_state;

fn terminal_tool_call(index: u64, tool: &ToolCallBuffer, content: &[Value]) -> Option<Value> {
    let block = content
        .iter()
        .find(|block| block.get("id").and_then(Value::as_str) == Some(&tool.id))
        .or_else(|| content.get(index as usize))?;
    if block.get("type").and_then(Value::as_str) != Some("toolCall") {
        return None;
    }
    Some(json!({
        "index":index,
        "toolCallId":block.get("id").cloned().unwrap_or_else(|| json!(tool.id)),
        "name":block.get("name").cloned().unwrap_or_else(|| json!(tool.name)),
        "arguments":block.get("arguments").cloned().unwrap_or_else(|| json!({}))
    }))
}

impl PiGateway {
    pub(super) fn handle_event(
        &self,
        thread_id: &str,
        request_id: &str,
        event: &Value,
        state: &mut EventTranslateState,
    ) -> Result<bool> {
        let event_type = protocol::validate_event(event, request_id)?;
        tracing::debug!(thread_id, request_id, event_type, event = %event, "received Pi gateway event");
        match event_type {
            "start" | "text_start" => {}
            "thinking_start" | "thinking_progress" => {
                self.dispatch_thinking_progress(thread_id, request_id, event);
            }
            "text_end" => {
                self.dispatch_end_content(thread_id, request_id, event, state)?;
            }
            "thinking_end" => {
                self.dispatch_thinking_result(thread_id, request_id, event, "content", state)?;
            }
            "thinking_result" => {
                self.dispatch_thinking_result(thread_id, request_id, event, "result", state)?;
            }
            "text_delta" => {
                mark_streamed(state, event);
                self.dispatch_delta(thread_id, request_id, event)?;
            }
            "thinking_delta" => {
                self.dispatch_thinking_progress(thread_id, request_id, event);
            }
            "toolcall_start" => {
                start_tool_call(event, &mut state.tools)?;
                self.emit_tool_start_if_ready(thread_id, request_id, event, state)?;
            }
            "toolcall_delta" => {
                append_tool_call(event, &mut state.tools)?;
                self.emit_tool_start_if_ready(thread_id, request_id, event, state)?;
                self.emit_tool_delta(thread_id, request_id, event, &state.tools)?;
            }
            "toolcall_end" => {
                self.finish_tool_call(thread_id, request_id, event, state)?;
            }
            "done" => return self.dispatch_done(thread_id, request_id, event, state),
            "error" | "protocol_error" => {
                self.dispatch_error_event(request_id, thread_id, event);
                return Ok(true);
            }
            other => bail!("unsupported Pi gateway event type `{other}`"),
        }
        Ok(false)
    }

    fn dispatch_done(
        &self,
        thread_id: &str,
        request_id: &str,
        event: &Value,
        state: &mut EventTranslateState,
    ) -> Result<bool> {
        self.finish_terminal_tool_calls(thread_id, request_id, event, state)?;
        let reported_stop_reason = anthropic_stop_reason(event)?;
        let provider_recoverable =
            event.pointer("/terminal/state").and_then(Value::as_str) == Some("recoverable_error");
        let missing_forwarded_tool =
            reported_stop_reason == "tool_use" && state.forwarded_tool_calls == 0;
        let recoverable = provider_recoverable || missing_forwarded_tool;
        let stop_reason = if recoverable {
            "end_turn"
        } else {
            reported_stop_reason
        };
        let recoverable_fallback = json!({
            "state":"recoverable_error",
            "output":"none",
            "code":"tool_use_without_forwarded_call"
        });
        let terminal = if missing_forwarded_tool {
            recoverable_fallback
        } else if provider_recoverable {
            event
                .get("terminal")
                .cloned()
                .unwrap_or(recoverable_fallback)
        } else {
            event
                .get("terminal")
                .cloned()
                .unwrap_or_else(|| json!({"state":"complete"}))
        };
        self.dispatch_usage(thread_id, request_id, event);
        self.events.dispatch_to(
            request_id,
            json!({
                "method":"turn/completed",
                "params":{"threadId":thread_id,"turn":{
                    "threadId":thread_id,
                    "status":"completed",
                    "providerStopReason":stop_reason,
                    "terminal":terminal
                }}
            }),
        );
        Ok(true)
    }

    fn finish_terminal_tool_calls(
        &self,
        thread_id: &str,
        request_id: &str,
        event: &Value,
        state: &mut EventTranslateState,
    ) -> Result<()> {
        let Some(content) = event.pointer("/message/content").and_then(Value::as_array) else {
            return Ok(());
        };
        let terminal_calls = state
            .tools
            .iter()
            .filter_map(|(index, tool)| terminal_tool_call(*index, tool, content))
            .collect::<Vec<_>>();
        for terminal_call in terminal_calls {
            self.finish_tool_call(thread_id, request_id, &terminal_call, state)?;
        }
        Ok(())
    }

    fn dispatch_error_event(&self, request_id: &str, thread_id: &str, event: &Value) {
        let message = event
            .pointer("/error/errorMessage")
            .or_else(|| event.pointer("/error/message"))
            .or_else(|| event.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("Pi gateway request failed");
        self.dispatch_error_to(request_id, thread_id, message);
    }
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
