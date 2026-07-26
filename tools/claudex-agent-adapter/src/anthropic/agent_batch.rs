use serde_json::{Value, json};

const MAPPED_NAME_PREFIX: &str = "__claudex_agent_batch__:";
const MARKER_KEY: &str = "claudexAgentBatch";
pub(super) const MAX_BATCH_SIZE: usize = 40;

pub(super) struct PendingBatch<'a> {
    pub(super) request_id: &'a Value,
    pub(super) index: usize,
    pub(super) total: usize,
}

pub(super) fn supports(tool_name: &str) -> bool {
    matches!(tool_name, "Agent" | "Task")
}

pub(super) fn mapped_name(tool_name: &str) -> String {
    format!("{MAPPED_NAME_PREFIX}{tool_name}")
}

pub(super) fn original_name(mapped: &str) -> Option<&str> {
    mapped.strip_prefix(MAPPED_NAME_PREFIX)
}

pub(super) fn dynamic_tool(tool: &Value, codex_name: &str) -> Option<Value> {
    let original_name = tool.get("name")?.as_str()?;
    let item_schema = crate::anthropic::agent_effort::tool_schema(
        original_name,
        tool.get("input_schema")
            .cloned()
            .unwrap_or_else(|| json!({"type":"object"})),
    );
    Some(json!({
        "type":"function",
        "name":codex_name,
        "description":format!(
            "Required instead of `{original_name}` when launching two or more independent SubAgents. Supply every intended launch in one tasks array; the bridge emits them concurrently in one Claude Code tool round."
        ),
        "inputSchema":{
            "type":"object",
            "properties":{"tasks":{"type":"array","minItems":2,"maxItems":MAX_BATCH_SIZE,"items":item_schema}},
            "required":["tasks"],
            "additionalProperties":false
        }
    }))
}

pub(super) fn pending_marker(request_id: Value, index: usize, total: usize) -> Value {
    json!({MARKER_KEY:{"requestId":request_id,"index":index,"total":total}})
}

pub(super) fn pending_batch(value: &Value) -> Option<PendingBatch<'_>> {
    let marker = value.get(MARKER_KEY)?;
    Some(PendingBatch {
        request_id: marker.get("requestId")?,
        index: marker.get("index")?.as_u64()?.try_into().ok()?,
        total: marker.get("total")?.as_u64()?.try_into().ok()?,
    })
}
