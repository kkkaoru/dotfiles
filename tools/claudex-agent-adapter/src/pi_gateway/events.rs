use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::{PiGateway, protocol};

const TOOL_START_METHOD: &str = "item/tool/start";
const TOOL_DELTA_METHOD: &str = "item/tool/delta";

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
                self.emit_tool_start_if_ready(thread_id, request_id, event, &mut state.tools)?;
            }
            "toolcall_delta" => {
                append_tool_call(event, &mut state.tools)?;
                self.emit_tool_start_if_ready(thread_id, request_id, event, &mut state.tools)?;
                self.emit_tool_delta(thread_id, request_id, event, &state.tools)?;
            }
            "toolcall_end" => {
                self.finish_tool_call(thread_id, request_id, event, &mut state.tools)?;
            }
            "done" => {
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
                return Ok(true);
            }
            "error" | "protocol_error" => {
                let message = event
                    .pointer("/error/errorMessage")
                    .or_else(|| event.pointer("/error/message"))
                    .or_else(|| event.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Pi gateway request failed");
                self.dispatch_error_to(request_id, thread_id, message);
                return Ok(true);
            }
            other => bail!("unsupported Pi gateway event type `{other}`"),
        }
        Ok(false)
    }

    fn dispatch_delta(
        &self,
        thread_id: &str,
        request_id: &str,
        event: &Value,
        thinking: bool,
    ) -> Result<()> {
        let delta = event
            .get("delta")
            .and_then(Value::as_str)
            .context("Pi gateway delta event omitted delta")?;
        let method = if thinking {
            "item/reasoning/summaryTextDelta"
        } else {
            "item/agentMessage/delta"
        };
        let mut params = json!({
            "threadId":thread_id,
            "turnId":thread_id,
            "itemId":format!("pi-{}", event.get("index").and_then(Value::as_u64).unwrap_or(0)),
            "delta":delta
        });
        if thinking {
            params["summaryIndex"] = json!(0);
        }
        self.events
            .dispatch_to(request_id, json!({"method":method,"params":params}));
        Ok(())
    }

    fn dispatch_end_content(
        &self,
        thread_id: &str,
        request_id: &str,
        event: &Value,
        thinking: bool,
        state: &mut EventTranslateState,
    ) -> Result<()> {
        let index = event_index(event)?;
        if !state.streamed_content.insert(index) {
            return Ok(());
        }
        let Some(content) = event
            .get("content")
            .and_then(Value::as_str)
            .filter(|content| !content.is_empty())
        else {
            return Ok(());
        };
        self.dispatch_delta(
            thread_id,
            request_id,
            &json!({"index":index,"delta":content}),
            thinking,
        )
    }

    fn emit_tool_start_if_ready(
        &self,
        thread_id: &str,
        request_id: &str,
        event: &Value,
        tools: &mut HashMap<u64, ToolCallBuffer>,
    ) -> Result<()> {
        let index = event_index(event)?;
        let Some(tool) = tools.get_mut(&index) else {
            return Ok(());
        };
        if let Some(id) = tool_id(event).filter(|id| !id.is_empty()) {
            tool.id = id.to_owned();
        }
        if let Some(name) = tool_name(event).filter(|name| !name.is_empty()) {
            tool.name = name.to_owned();
        }
        if tool.start_emitted || tool.id.is_empty() || tool.name.is_empty() {
            return Ok(());
        }
        tool.start_emitted = true;
        self.events.dispatch_to(
            request_id,
            json!({
                "id":tool.id,
                "method":TOOL_START_METHOD,
                "params":{
                    "threadId":thread_id,
                    "callId":tool.id,
                    "tool":tool.name
                }
            }),
        );
        Ok(())
    }

    fn emit_tool_delta(
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

    fn finish_tool_call(
        &self,
        thread_id: &str,
        request_id: &str,
        event: &Value,
        tools: &mut HashMap<u64, ToolCallBuffer>,
    ) -> Result<()> {
        let index = event_index(event)?;
        let mut tool = tools
            .remove(&index)
            .context("Pi toolcall_end did not match toolcall_start")?;
        if let Some(id) = tool_id(event) {
            tool.id = id.to_owned();
        }
        if let Some(name) = tool_name(event) {
            tool.name = name.to_owned();
        }
        let arguments = event
            .pointer("/toolCall/arguments")
            .or_else(|| event.get("arguments"))
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| {
                serde_json::from_str(&tool.arguments).context("decode Pi tool call arguments")
            })?;
        if tool.id.is_empty() || tool.name.is_empty() {
            bail!("Pi tool call omitted id or name");
        }
        self.events.dispatch_to(
            request_id,
            json!({
                "id":tool.id,
                "method":"item/tool/call",
                "params":{
                    "threadId":thread_id,
                    "callId":tool.id,
                    "tool":tool.name,
                    "arguments":arguments
                }
            }),
        );
        Ok(())
    }

    fn dispatch_usage(&self, thread_id: &str, request_id: &str, event: &Value) {
        let usage = event.pointer("/message/usage").unwrap_or(&Value::Null);
        let output = usage.get("output").and_then(Value::as_u64).unwrap_or(0);
        let reasoning = usage
            .get("reasoning")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(output);
        let cache_creation = usage.get("cacheWrite").and_then(Value::as_u64).unwrap_or(0);
        let mut last = json!({
            "inputTokens":usage.get("input").and_then(Value::as_u64).unwrap_or(0),
            "outputTokens":output - reasoning,
            "reasoningOutputTokens":reasoning,
            "cacheReadInputTokens":usage.get("cacheRead").and_then(Value::as_u64).unwrap_or(0),
            "cacheCreationInputTokens":cache_creation
        });
        if let Some(one_hour) = usage.get("cacheWrite1h").and_then(Value::as_u64) {
            last["cacheCreation1hInputTokens"] = json!(one_hour.min(cache_creation));
        }
        if let Some(total) = usage.get("totalTokens").and_then(Value::as_u64) {
            last["totalTokens"] = json!(total);
        }
        if let Some(cost) = usage.get("cost").filter(|cost| cost.is_object()) {
            last["cost"] = cost.clone();
        }
        self.events.dispatch_to(
            request_id,
            json!({
                "method":"thread/tokenUsage/updated",
                "params":{"threadId":thread_id,"tokenUsage":{"last":last}}
            }),
        );
    }

    pub(super) fn dispatch_error_to(&self, request_id: &str, thread_id: &str, message: &str) {
        self.events.dispatch_to(
            request_id,
            json!({
                "method":"error",
                "params":{"threadId":thread_id,"willRetry":false,"error":{"message":message}}
            }),
        );
    }
}

fn anthropic_stop_reason(event: &Value) -> Result<&'static str> {
    match event.get("reason").and_then(Value::as_str) {
        Some("stop") => Ok("end_turn"),
        Some("length") => Ok("max_tokens"),
        Some("toolUse") => Ok("tool_use"),
        Some("deferred") => Ok("pause_turn"),
        Some(reason) => bail!("unsupported Pi gateway stop reason `{reason}`"),
        None => bail!("Pi gateway done event omitted reason"),
    }
}

fn event_index(event: &Value) -> Result<u64> {
    event
        .get("index")
        .or_else(|| event.get("contentIndex"))
        .and_then(Value::as_u64)
        .context("Pi gateway content event omitted index")
}

fn tool_id(event: &Value) -> Option<&str> {
    event
        .get("toolCallId")
        .or_else(|| event.pointer("/toolCall/id"))
        .or_else(|| event.pointer("/block/id"))
        .and_then(Value::as_str)
}

fn tool_name(event: &Value) -> Option<&str> {
    event
        .get("name")
        .or_else(|| event.pointer("/toolCall/name"))
        .or_else(|| event.pointer("/block/name"))
        .and_then(Value::as_str)
}

fn start_tool_call(event: &Value, tools: &mut HashMap<u64, ToolCallBuffer>) -> Result<()> {
    let index = event_index(event)?;
    if tools.contains_key(&index) {
        bail!("Pi gateway repeated toolcall_start index {index}");
    }
    tools.insert(
        index,
        ToolCallBuffer {
            id: tool_id(event).unwrap_or_default().to_owned(),
            name: tool_name(event).unwrap_or_default().to_owned(),
            arguments: String::new(),
            start_emitted: false,
        },
    );
    Ok(())
}

fn mark_streamed(state: &mut EventTranslateState, event: &Value) {
    if let Ok(index) = event_index(event) {
        state.streamed_content.insert(index);
    }
}

fn append_tool_call(event: &Value, tools: &mut HashMap<u64, ToolCallBuffer>) -> Result<()> {
    let index = event_index(event)?;
    let delta = event
        .get("delta")
        .and_then(Value::as_str)
        .context("Pi toolcall_delta omitted delta")?;
    tools
        .get_mut(&index)
        .context("Pi toolcall_delta did not match toolcall_start")?
        .arguments
        .push_str(delta);
    Ok(())
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
