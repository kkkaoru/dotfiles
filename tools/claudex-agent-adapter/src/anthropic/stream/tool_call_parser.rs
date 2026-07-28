use anyhow::{Context, Result};
use serde_json::Value;

use super::ToolCall;

pub(super) fn parse_tool_call(event: &Value) -> Result<ToolCall<'_>> {
    let params = event.get("params").context("tool call params missing")?;
    Ok(ToolCall {
        call_id: params
            .get("callId")
            .and_then(Value::as_str)
            .context("tool call callId missing")?,
        name: params
            .get("tool")
            .and_then(Value::as_str)
            .context("tool call name missing")?,
        arguments: params.get("arguments").unwrap_or(&Value::Null),
        request_id: event
            .get("id")
            .cloned()
            .context("tool request id missing")?,
    })
}
