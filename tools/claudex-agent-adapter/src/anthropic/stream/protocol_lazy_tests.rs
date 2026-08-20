//! Coverage gates measure production stream framing; this module only contains tests.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use super::*;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_stream::StreamExt;

#[tokio::test]
async fn absent_stream_does_not_build_frame() {
    let built = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&built);

    send_stream_frame(None, "ignored", || mark_frame_built(&observed))
        .await
        .expect("optional stream");

    assert!(!built.load(Ordering::Relaxed));
}

#[tokio::test]
async fn marks_missing_provider_environment_as_non_retryable() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);

    send_stream_error(
        &sender,
        anyhow::anyhow!("Missing environment variable: SAKANA_AI_API_KEY"),
    )
    .await;
    drop(sender);

    let output = drain_frames(&mut receiver).await;
    assert!(output.contains("invalid_request_error"));
    assert!(output.contains("\"stop_reason\":\"error\""));
    assert!(output.contains("event: message_stop"));
}

#[tokio::test]
async fn graceful_stop_emits_end_turn_without_error_event() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);

    send_stream_graceful_stop(&sender, "Provider completed with no assistant content").await;
    drop(sender);

    let output = drain_frames(&mut receiver).await;
    assert!(output.contains("\"type\":\"text_delta\""));
    assert!(output.contains("Provider completed with no assistant content"));
    assert!(output.contains("\"stop_reason\":\"end_turn\""));
    assert!(output.contains("event: message_stop"));
    assert!(!output.contains("event: error"));
    assert!(!output.contains("api_error"));
}

#[tokio::test]
async fn visible_failure_emits_assistant_text_then_end_turn() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);

    send_stream_graceful_stop_at(&sender, 0, "Provider completed with no assistant content").await;
    drop(sender);

    let output = drain_frames(&mut receiver).await;
    assert!(output.contains("\"type\":\"text\""));
    assert!(output.contains("Provider completed with no assistant content"));
    assert!(output.contains("\"stop_reason\":\"end_turn\""));
    assert!(output.contains("event: message_stop"));
    assert!(!output.contains("event: error"));
}

#[tokio::test]
async fn primed_stream_without_blocks_emits_context_window_text() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    sender
        .try_send(Ok(Bytes::from(message_start("test-model", 138765))))
        .expect("prime message_start");
    send_empty_turn_or_overflow_error(
        &sender,
        0,
        &anyhow::anyhow!("context window exceeded: input_tokens 138765 > limit 110000"),
    )
    .await;
    drop(sender);

    let output = drain_frames(&mut receiver).await;
    assert!(output.contains("event: message_start"));
    assert!(output.contains("\"type\":\"text_delta\""));
    assert!(output.contains("Context window overflow after message_start; a retry was unavailable. No assistant content was produced. Compact or start a new turn; this is not successful work."));
    assert!(output.contains("context window exceeded: input_tokens 138765 > limit 110000"));
    assert!(output.contains("\"stop_reason\":\"end_turn\""));
    assert!(output.contains("event: message_stop"));
    assert!(!output.contains("event: error"));
}

#[tokio::test]
async fn primed_stream_without_blocks_emits_no_assistant_substitute() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    sender
        .try_send(Ok(Bytes::from(message_start("test-model", 12))))
        .expect("prime message_start");
    send_stream_graceful_stop(
        &sender,
        "No assistant content was produced after the stream started (no provider output or unusable tools). This is not a completed answer.",
    )
    .await;
    drop(sender);

    let output = drain_frames(&mut receiver).await;
    assert!(output.contains("event: message_start"));
    assert!(output.contains("\"type\":\"text_delta\""));
    assert!(output.contains("No assistant content was produced after the stream started (no provider output or unusable tools). This is not a completed answer."));
    assert!(output.contains("\"stop_reason\":\"end_turn\""));
    assert!(output.contains("event: message_stop"));
    assert!(!output.contains("event: error"));
}

#[tokio::test]
async fn empty_end_turn_completion_injects_visible_assistant_text() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    send_stream_completion(
        &sender,
        &super::Segment {
            blocks: Vec::new(),
            stop_reason: "end_turn",
            usage: super::super::super::Usage::default(),
            web_evidence: super::super::super::WebEvidenceSummary::default(),
            next_sse_index: 2,
        },
    )
    .await;
    drop(sender);

    let output = drain_frames(&mut receiver).await;
    assert!(output.contains("event: content_block_start"));
    assert!(output.contains("\"index\":2"));
    assert!(output.contains("\"type\":\"text_delta\""));
    assert!(output.contains("Provider completed with no assistant content. The route returned no assistant text or tools. This is a failure, not a completed result."));
    assert!(output.contains("\"stop_reason\":\"end_turn\""));
    assert!(output.contains("event: message_stop"));
    assert!(!output.contains("event: error"));
}

#[tokio::test]
async fn overflow_notice_uses_reserved_sse_index() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    send_empty_turn_or_overflow_error(
        &sender,
        1,
        &anyhow::anyhow!("context window exceeded: input_tokens 138765 > limit 110000"),
    )
    .await;
    drop(sender);

    let output = drain_frames(&mut receiver).await;
    assert!(output.contains("\"index\":1"));
    assert!(output.contains("\"type\":\"text_delta\""));
    assert!(output.contains("Context window overflow after message_start; a retry was unavailable. No assistant content was produced. Compact or start a new turn; this is not successful work."));
    assert!(output.contains("input_tokens 138765 > limit 110000"));
    assert!(output.contains("\"stop_reason\":\"end_turn\""));
    assert!(output.contains("event: message_stop"));
    assert!(!output.contains("event: error"));
}

async fn drain_frames(receiver: &mut mpsc::Receiver<Result<Bytes, Infallible>>) -> String {
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible frame")));
    }
    output
}

#[tokio::test]
async fn shared_stream_emits_anthropic_pings_and_stops_after_completion() {
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(2);
    let mut stream = KeepaliveStream::new(receiver, Duration::from_millis(5));

    let ping = tokio::time::timeout(Duration::from_millis(100), stream.next())
        .await
        .expect("ping deadline")
        .expect("ping frame")
        .expect("infallible frame");
    assert_eq!(ping.as_ref(), b"event: ping\ndata: {\"type\":\"ping\"}\n\n");

    let completion = Bytes::from_static(b"event: message_stop\ndata: {}\n\n");
    sender
        .send(Ok(completion.clone()))
        .await
        .expect("completion receiver");
    drop(sender);
    assert_eq!(
        stream.next().await.expect("completion frame"),
        Ok(completion)
    );
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn prioritizes_ready_model_frames_over_pings() {
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    let delta = Bytes::from_static(b"event: content_block_delta\ndata: {}\n\n");
    sender
        .try_send(Ok(delta.clone()))
        .expect("queued model delta");
    let mut stream = KeepaliveStream::new(receiver, Duration::ZERO);

    assert_eq!(stream.next().await.expect("model delta"), Ok(delta));
}

#[tokio::test]
async fn shared_http_response_streams_repeated_anthropic_pings() {
    const PING_FRAME: &[u8] = b"event: ping\ndata: {\"type\":\"ping\"}\n\n";

    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    let receiver = Arc::new(tokio::sync::Mutex::new(Some(receiver)));
    let app = axum::Router::new().route(
        "/",
        axum::routing::get(move || take_ping_response(Arc::clone(&receiver))),
    );
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return,
        Err(error) => panic!("bind SSE listener: {error}"),
    };
    let address = listener.local_addr().expect("SSE listener address");
    let server = tokio::spawn(serve_sse(listener, app));

    let mut client = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect SSE client");
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .expect("send SSE request");
    let wire = read_until_ping_count(&mut client, PING_FRAME, 2).await;
    assert!(wire.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(count_frames(&wire, PING_FRAME) >= 2);
    drop(sender);
    server.abort();
    let _ = server.await;
}

type PingReceiver = Arc<tokio::sync::Mutex<Option<mpsc::Receiver<Result<Bytes, Infallible>>>>>;

async fn take_ping_response(receiver: PingReceiver) -> Response<Body> {
    let receiver = receiver
        .lock()
        .await
        .take()
        .expect("single streaming request");
    streaming_sse_response_with_interval(receiver, Duration::from_millis(5))
}

async fn read_until_ping_count(
    client: &mut tokio::net::TcpStream,
    ping_frame: &[u8],
    needed: usize,
) -> Vec<u8> {
    tokio::time::timeout(
        Duration::from_secs(1),
        read_ping_frames(client, ping_frame, needed),
    )
    .await
    .expect("enough ping frames")
}

fn mark_frame_built(observed: &AtomicBool) -> Value {
    observed.store(true, Ordering::Relaxed);
    json!({})
}

async fn serve_sse(listener: tokio::net::TcpListener, app: axum::Router) {
    axum::serve(listener, app)
        .await
        .expect("serve SSE response");
}

async fn read_ping_frames(
    client: &mut tokio::net::TcpStream,
    ping_frame: &[u8],
    needed: usize,
) -> Vec<u8> {
    let mut wire = Vec::new();
    let mut chunk = [0; 1024];
    while count_frames(&wire, ping_frame) < needed {
        let count = client.read(&mut chunk).await.expect("read SSE response");
        assert_ne!(count, 0, "SSE response ended before enough pings");
        wire.extend_from_slice(&chunk[..count]);
    }
    wire
}

fn count_frames(wire: &[u8], frame: &[u8]) -> usize {
    wire.windows(frame.len())
        .filter(|window| *window == frame)
        .count()
}
