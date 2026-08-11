use std::convert::Infallible;

use anyhow::{Error, Result};
use axum::body::Bytes;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::super::{content::estimated_tokens, stream::send_stream_frame};

pub(in crate::anthropic) async fn send_tool_block(
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

pub(in crate::anthropic) async fn send_block_stop(
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

pub(in crate::anthropic) async fn send_tool_finish(
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

pub(in crate::anthropic) fn result_output_tokens(result: &Value) -> u64 {
    result
        .pointer("/usage/output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            estimated_tokens(
                result
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
        })
}

pub(in crate::anthropic) async fn send_subscription_error(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    error: Error,
) {
    let error_type = crate::anthropic::error::error_type(&error);
    let _ = send_stream_frame(Some(sender), "error", || {
        json!({
            "type":"error",
            "error":{"type":error_type,"message":format!("{error:#}")}
        })
    })
    .await;
}
