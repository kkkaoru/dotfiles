use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::events_finish::event_index;
use super::{EventTranslateState, PiGateway};

impl PiGateway {
    pub(super) fn dispatch_delta(
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

    pub(super) fn dispatch_end_content(
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

    pub(super) fn dispatch_usage(&self, thread_id: &str, request_id: &str, event: &Value) {
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

    pub(in crate::pi_gateway) fn dispatch_error_to(
        &self,
        request_id: &str,
        thread_id: &str,
        message: &str,
    ) {
        self.events.dispatch_to(
            request_id,
            json!({
                "method":"error",
                "params":{"threadId":thread_id,"willRetry":false,"error":{"message":message}}
            }),
        );
    }
}
