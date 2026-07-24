use std::convert::Infallible;

use anyhow::Result;
use axum::body::Bytes;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{agent_effort::is_agent_tool, stream::send_stream_frame};

pub(super) fn mapped_tool_name<'a>(emitted: &'a str, available: &'a [String]) -> &'a str {
    if is_agent_tool(emitted) {
        return available
            .iter()
            .find(|name| is_agent_tool(name))
            .map(String::as_str)
            .unwrap_or(emitted);
    }
    available
        .iter()
        .find(|name| name.as_str() == emitted)
        .map(String::as_str)
        .unwrap_or(emitted)
}

pub(super) fn assistant_output_tokens(envelope: &Value) -> u64 {
    envelope
        .pointer("/message/usage/output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

pub(super) async fn send_text_start(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    index: usize,
) -> Result<()> {
    send_stream_frame(Some(sender), "content_block_start", || {
        json!({
            "type":"content_block_start", "index":index,
            "content_block":{"type":"text","text":""}
        })
    })
    .await
}

pub(super) async fn send_text_delta(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    text: &str,
) -> Result<()> {
    send_stream_frame(Some(sender), "content_block_delta", || {
        json!({
            "type":"content_block_delta", "index":0,
            "delta":{"type":"text_delta","text":text}
        })
    })
    .await
}

pub(super) async fn send_text_finish(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    index: usize,
    output_tokens: u64,
) -> Result<()> {
    send_block_stop(sender, index).await?;
    for (event, frame) in [
        (
            "message_delta",
            json!({
                "type":"message_delta",
                "delta":{"stop_reason":"end_turn","stop_sequence":null},
                "usage":{"output_tokens":output_tokens}
            }),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ] {
        send_stream_frame(Some(sender), event, || frame).await?;
    }
    Ok(())
}

pub(super) async fn send_tool_block(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    index: usize,
    id: &str,
    name: &str,
    input: Value,
) -> Result<()> {
    send_stream_frame(Some(sender), "content_block_start", || {
        json!({
            "type":"content_block_start", "index":index,
            "content_block":{"type":"tool_use", "id":id, "name":name, "input":{}}
        })
    })
    .await?;
    let partial_json = serde_json::to_string(&input)?;
    send_stream_frame(Some(sender), "content_block_delta", || {
        json!({
            "type":"content_block_delta", "index":index,
            "delta":{"type":"input_json_delta", "partial_json":partial_json}
        })
    })
    .await?;
    send_block_stop(sender, index).await
}

pub(super) async fn send_block_stop(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    index: usize,
) -> Result<()> {
    send_stream_frame(
        Some(sender),
        "content_block_stop",
        || json!({"type":"content_block_stop", "index":index}),
    )
    .await
}

pub(super) async fn send_tool_finish(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    output_tokens: u64,
) -> Result<()> {
    for (event, frame) in [
        (
            "message_delta",
            json!({
                "type":"message_delta",
                "delta":{"stop_reason":"tool_use","stop_sequence":null},
                "usage":{"output_tokens":output_tokens}
            }),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ] {
        send_stream_frame(Some(sender), event, || frame).await?;
    }
    Ok(())
}
