use std::convert::Infallible;

use anyhow::Result;
use axum::body::Bytes;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{agent_effort::is_agent_tool, stream::send_stream_frame};

pub(super) fn subscription_start_frame(model: &str, input_tokens: u64) -> String {
    super::content::sse(
        "message_start",
        json!({
            "type":"message_start",
            "message":{
                "id":format!("msg_{}", Uuid::new_v4().simple()),
                "type":"message","role":"assistant","model":model,
                "content":[],"stop_reason":null,"stop_sequence":null,
                "usage":{"input_tokens":input_tokens,"output_tokens":0}
            }
        }),
    )
}

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
    index: usize,
    text: &str,
) -> Result<()> {
    send_stream_frame(Some(sender), "content_block_delta", || {
        json!({
            "type":"content_block_delta", "index":index,
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

#[path = "subscription_frames_tools.rs"]
mod tools;
pub(super) use tools::{
    result_output_tokens, send_block_stop, send_subscription_error, send_tool_block, send_tool_finish,
};

#[cfg(test)]
#[path = "subscription_frames_tests.rs"]
mod tests;
