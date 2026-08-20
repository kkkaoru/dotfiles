use anyhow::Result;
use axum::body::Bytes;
use serde_json::{Value, json};

use super::super::super::content::sse;
use super::StreamSender;

pub(in crate::anthropic) async fn send_stream_frame(
    stream: Option<&StreamSender>,
    event: &str,
    value: impl FnOnce() -> Value,
) -> Result<()> {
    let Some(sender) = stream else {
        return Ok(());
    };
    if sender
        .send(Ok(Bytes::from(sse(event, value()))))
        .await
        .is_err()
    {
        tracing::debug!(event, "Claude Code closed the streaming response");
    }
    Ok(())
}

pub(in crate::anthropic) async fn send_tool_use(
    stream: Option<&StreamSender>,
    index: usize,
    block: &Value,
) -> Result<()> {
    for (event, frame) in tool_use_frames(index, block) {
        send_stream_frame(stream, event, || frame).await?;
    }
    Ok(())
}

pub(in crate::anthropic) async fn send_tool_use_start(
    stream: Option<&StreamSender>,
    index: usize,
    id: &str,
    name: &str,
) -> Result<()> {
    send_stream_frame(stream, "content_block_start", || {
        json!({
            "type":"content_block_start", "index":index,
            "content_block":{"type":"tool_use","id":id,"name":name,"input":{}}
        })
    })
    .await
}

pub(in crate::anthropic) async fn send_input_json_delta(
    stream: Option<&StreamSender>,
    index: usize,
    partial_json: &str,
) -> Result<()> {
    if partial_json.is_empty() {
        return Ok(());
    }
    send_stream_frame(stream, "content_block_delta", || {
        json!({
            "type":"content_block_delta", "index":index,
            "delta":{"type":"input_json_delta","partial_json":partial_json}
        })
    })
    .await
}

pub(in crate::anthropic) async fn send_content_block_stop(
    stream: Option<&StreamSender>,
    index: usize,
) -> Result<()> {
    send_stream_frame(
        stream,
        "content_block_stop",
        || json!({"type":"content_block_stop","index":index}),
    )
    .await
}

/// Visible assistant text so a primed SSE turn cannot close with zero blocks.
pub(in crate::anthropic::stream) async fn send_visible_assistant_text(
    stream: Option<&StreamSender>,
    index: usize,
    text: &str,
) {
    let _ = send_stream_frame(stream, "content_block_start", || {
        json!({
            "type":"content_block_start",
            "index":index,
            "content_block":{"type":"text","text":""}
        })
    })
    .await;
    let _ = send_stream_frame(stream, "content_block_delta", || {
        json!({
            "type":"content_block_delta",
            "index":index,
            "delta":{"type":"text_delta","text":text}
        })
    })
    .await;
    let _ = send_content_block_stop(stream, index).await;
}

pub(in crate::anthropic) fn tool_use_frames(
    index: usize,
    block: &Value,
) -> [(&'static str, Value); 3] {
    [
        (
            "content_block_start",
            json!({
                "type":"content_block_start", "index":index,
                "content_block":{"type":"tool_use","id":block["id"],"name":block["name"],"input":{}}
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type":"content_block_delta", "index":index,
                "delta":{
                    "type":"input_json_delta",
                    "partial_json":block["input"].to_string()
                }
            }),
        ),
        (
            "content_block_stop",
            json!({"type":"content_block_stop","index":index}),
        ),
    ]
}
