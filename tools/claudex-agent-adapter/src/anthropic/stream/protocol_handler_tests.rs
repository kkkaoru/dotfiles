use std::convert::Infallible;
use tokio::sync::mpsc;
use serde_json::json;
use axum::body::Bytes;

#[tokio::test]
async fn send_stream_frame_gracefully_handles_closed_receiver() {
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(1);
    drop(receiver); // Receiver is closed, sender will fail

    let result = super::send_stream_frame(
        Some(&sender),
        "test_event",
        || json!({"type": "test"}),
    )
    .await;

    // Should return Ok(()) even if send fails (channel closed)
    // Error is logged but doesn't propagate
    assert!(result.is_ok());
}
