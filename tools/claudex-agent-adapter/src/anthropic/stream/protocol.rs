use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use anyhow::Result;
use axum::{
    body::{Body, Bytes},
    http::{Response, StatusCode, header},
};
use serde_json::{Value, json};
use tokio::{
    sync::mpsc,
    time::{Instant, Sleep, sleep},
};
use tokio_stream::Stream;
use uuid::Uuid;

use super::super::{Segment, content::sse};

// Anthropic `ping` frames keep Claude Code's ~180s raw-byte idle watchdog
// happy. They do NOT reset the ~300s decoded-event idle watchdog; that path
// needs content_block_delta heartbeats from wait_for_stream_segment.
// Keep raw-byte pings ahead of Claude Code's idle watchdog under load.
const SSE_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);
const SSE_KEEPALIVE_FRAME: &[u8] = b"event: ping\ndata: {\"type\":\"ping\"}\n\n";

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
    streaming_sse_response(receiver)
}

pub(in crate::anthropic) fn streaming_sse_response(
    receiver: mpsc::Receiver<Result<Bytes, Infallible>>,
) -> Response<Body> {
    streaming_sse_response_with_interval(receiver, SSE_KEEPALIVE_INTERVAL)
}

fn streaming_sse_response_with_interval(
    receiver: mpsc::Receiver<Result<Bytes, Infallible>>,
    interval: Duration,
) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(KeepaliveStream::new(receiver, interval)))
        .expect("valid streaming response")
}

struct KeepaliveStream {
    receiver: mpsc::Receiver<Result<Bytes, Infallible>>,
    interval: Duration,
    deadline: Pin<Box<Sleep>>,
}

impl KeepaliveStream {
    fn new(receiver: mpsc::Receiver<Result<Bytes, Infallible>>, interval: Duration) -> Self {
        Self {
            receiver,
            interval,
            deadline: Box::pin(sleep(interval)),
        }
    }

    fn reset_deadline(&mut self) {
        self.deadline.as_mut().reset(Instant::now() + self.interval);
    }
}

impl Stream for KeepaliveStream {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = self.get_mut();
        match stream.receiver.poll_recv(context) {
            Poll::Ready(Some(frame)) => {
                stream.reset_deadline();
                return Poll::Ready(Some(frame));
            }
            Poll::Ready(None) => return Poll::Ready(None),
            Poll::Pending => {}
        }
        if stream.deadline.as_mut().poll(context).is_ready() {
            stream.reset_deadline();
            return Poll::Ready(Some(Ok(Bytes::from_static(SSE_KEEPALIVE_FRAME))));
        }
        Poll::Pending
    }
}

pub(in crate::anthropic) async fn send_stream_completion(sender: &StreamSender, segment: &Segment) {
    let _ = send_stream_frame(Some(sender), "message_delta", || {
        let mut frame = json!({
            "type":"message_delta",
            "delta":{"stop_reason":segment.stop_reason,"stop_sequence":null},
            "usage":{
                "output_tokens":segment.usage.output_tokens,
                "server_tool_use":{"web_search_requests":segment.usage.web_search_requests}
            }
        });
        if let Some(metadata) = segment.web_evidence.metadata() {
            frame["metadata"] = metadata;
        }
        frame
    })
    .await;
    let _ = send_stream_frame(
        Some(sender),
        "message_stop",
        || json!({"type":"message_stop"}),
    )
    .await;
}

pub(super) async fn send_stream_error(sender: &StreamSender, error: anyhow::Error) {
    let error_type = super::super::error::error_type(&error);
    let _ = send_stream_frame(Some(sender), "error", || {
        json!({
            "type":"error",
            "error":{"type":error_type,"message":format!("{error:#}")}
        })
    })
    .await;
    // Claude Code keeps a SubAgent card running after a mid-response API/ACP
    // drop unless the SSE also ends with a terminal stop_reason + message_stop.
    let _ = send_stream_frame(Some(sender), "message_delta", || {
        json!({
            "type":"message_delta",
            "delta":{"stop_reason":"error","stop_sequence":null},
            "usage":{"output_tokens":0,"server_tool_use":{"web_search_requests":0}}
        })
    })
    .await;
    let _ = send_stream_frame(
        Some(sender),
        "message_stop",
        || json!({"type":"message_stop"}),
    )
    .await;
}

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

pub(super) async fn send_tool_use(
    stream: Option<&StreamSender>,
    index: usize,
    block: &Value,
) -> Result<()> {
    let Some(sender) = stream else {
        return Ok(());
    };
    for (event, frame) in tool_use_frames(index, block) {
        send_stream_frame(Some(sender), event, || frame).await?;
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "protocol_lazy_tests.rs"]
mod lazy_tests;


#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "protocol_handler_tests.rs"]
mod handler_tests;
