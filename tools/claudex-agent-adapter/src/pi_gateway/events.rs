use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::{PiGateway, protocol};

#[derive(Default)]
pub(super) struct ToolCallBuffer {
    id: String,
    name: String,
    arguments: String,
}

impl PiGateway {
    pub(super) fn handle_event(
        &self,
        thread_id: &str,
        request_id: &str,
        event: &Value,
        tools: &mut HashMap<u64, ToolCallBuffer>,
    ) -> Result<bool> {
        let event_type = protocol::validate_event(event, request_id)?;
        tracing::debug!(thread_id, request_id, event_type, event = %event, "received Pi gateway event");
        match event_type {
            "start" | "text_start" | "text_end" | "thinking_start" | "thinking_end" => {}
            "text_delta" => self.dispatch_delta(thread_id, request_id, event, false)?,
            "thinking_delta" => self.dispatch_delta(thread_id, request_id, event, true)?,
            "toolcall_start" => start_tool_call(event, tools)?,
            "toolcall_delta" => append_tool_call(event, tools)?,
            "toolcall_end" => self.finish_tool_call(thread_id, request_id, event, tools)?,
            "done" => {
                self.dispatch_usage(thread_id, request_id, event);
                self.events.dispatch_to(request_id, json!({
                    "method":"turn/completed",
                    "params":{"threadId":thread_id,"turn":{"threadId":thread_id,"status":"completed"}}
                }));
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
        self.events.dispatch_to(
            request_id,
            json!({
                "method":"thread/tokenUsage/updated",
                "params":{"threadId":thread_id,"tokenUsage":{"last":{
                    "inputTokens":usage.get("input").and_then(Value::as_u64).unwrap_or(0),
                    "outputTokens":usage.get("output").and_then(Value::as_u64).unwrap_or(0),
                    "reasoningOutputTokens":0
                }}}
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
        },
    );
    Ok(())
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
