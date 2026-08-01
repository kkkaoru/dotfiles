use serde_json::Value;

pub(super) fn coalescible_suffix<'a>(last: &Value, next: &'a Value) -> Option<&'a str> {
    let method = last.get("method")?.as_str()?;
    if next.get("method")?.as_str()? != method
        || !matches!(
            method,
            "item/agentMessage/delta" | "item/reasoning/summaryTextDelta"
        )
        || last.pointer("/params/turnId") != next.pointer("/params/turnId")
        || last.pointer("/params/itemId") != next.pointer("/params/itemId")
        || (method == "item/reasoning/summaryTextDelta"
            && last.pointer("/params/summaryIndex") != next.pointer("/params/summaryIndex"))
    {
        return None;
    }
    last.pointer("/params/delta")?.as_str()?;
    next.pointer("/params/delta")?.as_str()
}

pub(super) fn is_bridge_event(event: &Value) -> bool {
    // App-server lifecycle events can repeat the complete user input and dynamic tool schemas.
    // The Anthropic bridge ignores them, so admitting them can overflow the queue before the
    // small output deltas behind them are consumed.
    match event.get("method").and_then(Value::as_str) {
        Some("item/started" | "item/completed") => {
            event.pointer("/params/item/type").and_then(Value::as_str) == Some("webSearch")
        }
        Some(
            "item/agentMessage/delta"
            | "item/reasoning/summaryTextDelta"
            | "item/tool/call"
            | "item/providerTool/call"
            | "item/providerTool/update"
            | "thread/tokenUsage/updated"
            | "turn/completed"
            | "error",
        ) => true,
        _ => false,
    }
}

pub(super) fn is_terminal_event(event: &Value) -> bool {
    event.get("method").and_then(Value::as_str) == Some("turn/completed")
        && event.pointer("/params/turn/status").and_then(Value::as_str) != Some("inProgress")
}

pub(super) fn event_thread_id(event: &Value) -> Option<&str> {
    event
        .pointer("/params/threadId")
        .and_then(Value::as_str)
        .or_else(|| {
            event
                .pointer("/params/turn/threadId")
                .and_then(Value::as_str)
        })
}
