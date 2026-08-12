use serde_json::Value;
use uuid::Uuid;

pub(super) fn thinking_signature(item_id: &str) -> String {
    match item_id {
        "claudex_provider_progress" | "claudex_activity_keepalive" => item_id.to_owned(),
        _ => format!("claudex_local_{}", Uuid::new_v4().simple()),
    }
}

#[rustfmt::skip]
pub(super) fn summary_delta(event: &Value) -> Option<(&str, i64, &str)> {
    let params = event.get("params")?;
    let summary_index = params.get("summaryIndex");
    let content_index = params.get("contentIndex");
    let index = summary_index.and_then(Value::as_i64)
        .or_else(|| content_index.and_then(Value::as_i64))
        .or_else(|| {
            (summary_index.is_none()
                && content_index.is_none()
                && event.get("method").and_then(Value::as_str)
                    == Some("item/reasoning/textDelta"))
            .then_some(0)
        })?;
    Some((params.get("itemId")?.as_str()?, index, params.get("delta")?.as_str()?))
}

pub(super) fn has_visible_output(blocks: &[Value]) -> bool {
    blocks.iter().any(|block| {
        !matches!(
            block.get("type").and_then(Value::as_str),
            Some("thinking" | "server_tool_use")
        )
    })
}

pub(super) fn has_answer_text(blocks: &[Value]) -> bool {
    blocks
        .iter()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("text"))
}
