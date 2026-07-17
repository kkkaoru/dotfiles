use std::convert::Infallible;

use anyhow::Result;
use axum::{
    body::{Body, Bytes},
    http::{Response, StatusCode, header},
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use super::super::{Segment, content::sse};

pub(in crate::anthropic) type StreamSender = mpsc::Sender<Result<Bytes, Infallible>>;

pub(in crate::anthropic) fn message_start(model: &str, input_tokens: u64) -> String {
    sse(
        "message_start",
        json!({
            "type":"message_start",
            "message": {
                "id":format!("msg_{}", Uuid::new_v4().simple()),
                "type":"message", "role":"assistant", "model":model,
                "content":[], "stop_reason":null, "stop_sequence":null,
                "usage":{"input_tokens":input_tokens,"output_tokens":0}
            }
        }),
    )
}

pub(super) fn sse_response(receiver: mpsc::Receiver<Result<Bytes, Infallible>>) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(ReceiverStream::new(receiver)))
        .expect("valid streaming response")
}

pub(super) async fn send_stream_completion(sender: &StreamSender, segment: &Segment) {
    let _ = send_stream_frame(
        Some(sender),
        "message_delta",
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":segment.stop_reason,"stop_sequence":null},
            "usage":{"output_tokens":segment.usage.output_tokens}
        }),
    )
    .await;
    let _ = send_stream_frame(Some(sender), "message_stop", json!({"type":"message_stop"})).await;
}

pub(super) async fn send_stream_error(sender: &StreamSender, error: anyhow::Error) {
    let _ = send_stream_frame(
        Some(sender),
        "error",
        json!({
            "type":"error",
            "error":{"type":"api_error","message":format!("{error:#}")}
        }),
    )
    .await;
}

pub(in crate::anthropic) async fn send_stream_frame(
    stream: Option<&StreamSender>,
    event: &str,
    value: Value,
) -> Result<()> {
    if let Some(sender) = stream
        && sender
            .send(Ok(Bytes::from(sse(event, value))))
            .await
            .is_err()
    {
        tracing::debug!(event, "Claude Code closed the streaming response");
    }
    Ok(())
}

pub(super) async fn send_tool_use(
    stream: Option<&StreamSender>,
    index: usize,
    block: &Value,
) -> Result<()> {
    for (event, frame) in tool_use_frames(index, block) {
        send_stream_frame(stream, event, frame).await?;
    }
    Ok(())
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
