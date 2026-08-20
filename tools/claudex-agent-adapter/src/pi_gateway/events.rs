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
    listed_tools: HashSet<String>,
    consecutive_unusable_tools: u8,
}
pub(super) use events_finish::event_translate_state;

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
            "start" | "text_start" | "thinking_start" => {}
            "text_end" => {
                self.dispatch_end_content(thread_id, request_id, event, false, state)?;
            }
            "thinking_end" => {
                self.dispatch_end_content(thread_id, request_id, event, true, state)?;
                self.events.dispatch_to(
                    request_id,
                    json!({
                        "method":"item/reasoning/complete",
                        "params":{"threadId":thread_id}
                    }),
                );
            }
            "text_delta" => {
                mark_streamed(state, event);
                self.dispatch_delta(thread_id, request_id, event, false)?;
            }
            "thinking_delta" => {
                mark_streamed(state, event);
                self.dispatch_delta(thread_id, request_id, event, true)?;
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
            "done" => return self.dispatch_done(thread_id, request_id, event),
            "error" | "protocol_error" => {
                self.dispatch_error_event(request_id, thread_id, event);
                return Ok(true);
            }
            other => bail!("unsupported Pi gateway event type `{other}`"),
        }
        Ok(false)
    }

    fn dispatch_done(&self, thread_id: &str, request_id: &str, event: &Value) -> Result<bool> {
        let stop_reason = anthropic_stop_reason(event)?;
        self.dispatch_usage(thread_id, request_id, event);
        self.events.dispatch_to(
            request_id,
            json!({
                "method":"turn/completed",
                "params":{"threadId":thread_id,"turn":{
                    "threadId":thread_id,
                    "status":"completed",
                    "providerStopReason":stop_reason
                }}
            }),
        );
        Ok(true)
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
