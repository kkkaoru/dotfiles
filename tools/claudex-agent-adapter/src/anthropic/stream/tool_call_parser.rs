use anyhow::{Context, Result};
use serde_json::Value;

use super::ToolCall;

#[derive(Debug)]
pub(super) struct ToolStart {
    pub(super) call_id: String,
    pub(super) name: String,
}

#[derive(Debug)]
pub(super) struct ToolDelta {
    pub(super) call_id: String,
    pub(super) delta: String,
}

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

pub(super) fn parse_tool_start(event: &Value) -> Result<ToolStart> {
    let params = event.get("params").context("tool start params missing")?;
    Ok(ToolStart {
        call_id: params
            .get("callId")
            .and_then(Value::as_str)
            .context("tool start callId missing")?
            .to_owned(),
        name: params
            .get("tool")
            .and_then(Value::as_str)
            .context("tool start name missing")?
            .to_owned(),
    })
}

pub(super) fn parse_tool_delta(event: &Value) -> Result<ToolDelta> {
    let params = event.get("params").context("tool delta params missing")?;
    Ok(ToolDelta {
        call_id: params
            .get("callId")
            .and_then(Value::as_str)
            .context("tool delta callId missing")?
            .to_owned(),
        delta: params
            .get("delta")
            .and_then(Value::as_str)
            .context("tool delta missing")?
            .to_owned(),
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    #[test]
    fn parse_tool_start_requires_call_id_and_name() {
        let error = super::parse_tool_start(&json!({})).expect_err("params missing");
        assert!(error.to_string().contains("params missing"));
        let error = super::parse_tool_start(&json!({"params":{}})).expect_err("empty params");
        assert!(error.to_string().contains("callId missing"));
        let error =
            super::parse_tool_start(&json!({"params":{"callId":"c"}})).expect_err("name missing");
        assert!(error.to_string().contains("name missing"));
        let start = super::parse_tool_start(&json!({
            "params":{"callId":"c1","tool":"Read"}
        }))
        .expect("valid start");
        assert_eq!(start.call_id, "c1");
        assert_eq!(start.name, "Read");
    }

    #[test]
    fn parse_tool_delta_requires_call_id_and_delta() {
        let error = super::parse_tool_delta(&json!({})).expect_err("params missing");
        assert!(error.to_string().contains("params missing"));
        let error =
            super::parse_tool_delta(&json!({"params":{"callId":"c"}})).expect_err("delta missing");
        assert!(error.to_string().contains("delta missing"));
        let delta = super::parse_tool_delta(&json!({
            "params":{"callId":"c1","delta":"{\"a\":1}"}
        }))
        .expect("valid delta");
        assert_eq!(delta.call_id, "c1");
        assert_eq!(delta.delta, "{\"a\":1}");
    }
}
