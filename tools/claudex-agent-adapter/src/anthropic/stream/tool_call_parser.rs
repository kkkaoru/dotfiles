use anyhow::{Context, Result};
use serde_json::Value;

use super::ToolCall;

pub(super) fn parse_tool_call(event: &Value) -> Result<ToolCall> {
    let params = event.get("params").context("tool call params missing")?;
    Ok(ToolCall {
        call_id: params
            .get("callId")
            .and_then(Value::as_str)
            .context("tool call callId missing")?
            .to_owned(),
        name: params
            .get("tool")
            .and_then(Value::as_str)
            .context("tool call name missing")?
            .to_owned(),
        arguments: params.get("arguments").cloned().unwrap_or(Value::Null),
        request_id: event
            .get("id")
            .cloned()
            .context("tool request id missing")?,
    })
}
