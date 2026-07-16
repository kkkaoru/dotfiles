use std::{convert::Infallible, process::Stdio};

use axum::body::Bytes;
use serde_json::json;
use tokio::{process::Command, sync::mpsc};

use super::{
    SubscriptionStream, consume_subscription_stream, result_output_tokens, run_subscription_stream,
};
use crate::anthropic::subscription::SubscriptionOptions;

type Frame = Result<Bytes, Infallible>;
type FrameChannel = (mpsc::Sender<Frame>, mpsc::Receiver<Frame>);

fn channel() -> FrameChannel {
    mpsc::channel(16)
}

async fn output(receiver: &mut mpsc::Receiver<Result<Bytes, Infallible>>) -> String {
    let mut output = String::new();
    while let Ok(frame) = receiver.try_recv() {
        output.push_str(&String::from_utf8_lossy(&frame.expect("stream frame")));
    }
    output
}

#[tokio::test]
async fn handles_ignored_invalid_and_non_text_events() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        saw_result: false,
    };
    stream
        .handle_line(&sender, r#"{"type":"ignored"}"#)
        .await
        .expect("ignored envelope");
    stream
        .handle_line(
            &sender,
            r#"{"type":"stream_event","event":{"delta":{"type":"input_json_delta"}}}"#,
        )
        .await
        .expect("non-text delta");
    assert!(!stream.text_started);
    assert!(output(&mut receiver).await.is_empty());
    assert!(
        stream
            .handle_line(&sender, "not-json")
            .await
            .expect_err("invalid JSON")
            .to_string()
            .contains("invalid stream JSON")
    );
}

#[tokio::test]
async fn forwards_empty_and_regular_deltas_then_finishes_once() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        saw_result: false,
    };
    for text in ["", "hello"] {
        stream
            .handle_line(
                &sender,
                &json!({
                    "type":"stream_event",
                    "event":{"delta":{"type":"text_delta","text":text}}
                })
                .to_string(),
            )
            .await
            .expect("text delta");
    }
    stream
        .handle_line(
            &sender,
            r#"{"type":"result","subtype":"success","result":"fallback","usage":{"output_tokens":5}}"#,
        )
        .await
        .expect("result");
    assert!(stream.text_started);
    assert!(stream.saw_result);
    let output = output(&mut receiver).await;
    assert_eq!(output.matches("event: content_block_start").count(), 1);
    assert!(output.contains("hello"));
    assert!(!output.contains("fallback"));
    assert!(output.contains("\"output_tokens\":5"));
}

#[tokio::test]
async fn falls_back_to_result_text_and_estimated_tokens() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        saw_result: false,
    };
    stream
        .finish(
            &sender,
            &json!({"type":"result","subtype":"success","result":"fallback text"}),
        )
        .await
        .expect("fallback result");
    let output = output(&mut receiver).await;
    assert!(output.contains("fallback text"));
    assert!(result_output_tokens(&json!({"result":"four word result here"})) > 0);
    assert_eq!(
        result_output_tokens(&json!({"usage":{"output_tokens":17}})),
        17
    );
    assert_eq!(result_output_tokens(&json!({})), 0);
}

#[tokio::test]
async fn rejects_unsuccessful_results() {
    let (sender, _) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        saw_result: false,
    };
    assert!(
        stream
            .finish(
                &sender,
                &json!({"type":"result","subtype":"error","result":"bad"}),
            )
            .await
            .is_err()
    );
}

fn child(script: &str) -> tokio::process::Child {
    Command::new("sh")
        .args(["-c", script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stream fixture")
}

#[tokio::test]
async fn consumes_successful_process_and_requires_result_event() {
    let (sender, mut receiver) = channel();
    consume_subscription_stream(
        child(r#"printf '%s\n' '{"type":"result","subtype":"success","result":"done"}'"#),
        &sender,
    )
    .await
    .expect("successful subscription stream");
    assert!(output(&mut receiver).await.contains("done"));

    let error =
        consume_subscription_stream(child("printf '%s\\n' '{\"type\":\"ignored\"}'"), &sender)
            .await
            .expect_err("missing result");
    assert!(error.to_string().contains("without a result"));
}

#[tokio::test]
async fn reports_process_failure_and_stderr() {
    let (sender, _receiver) = channel();
    let error = consume_subscription_stream(child("printf 'fixture failure' >&2; exit 7"), &sender)
        .await
        .expect_err("failed process");
    let message = error.to_string();
    assert!(message.contains("fixture failure"));
    assert!(message.contains("exit status"));
}

#[tokio::test]
async fn stops_cleanly_when_the_receiver_closes() {
    let (sender, receiver) = channel();
    drop(receiver);
    consume_subscription_stream(child("sleep 1"), &sender)
        .await
        .expect("closed response stream");
}

#[tokio::test]
async fn requires_piped_stdout_and_stderr() {
    let (sender, _receiver) = channel();
    let missing_stdout = Command::new("sh")
        .args(["-c", "exit 0"])
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn without stdout");
    assert!(
        consume_subscription_stream(missing_stdout, &sender)
            .await
            .expect_err("missing stdout")
            .to_string()
            .contains("stdout is unavailable")
    );

    let missing_stderr = Command::new("sh")
        .args(["-c", "exit 0"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn without stderr");
    assert!(
        consume_subscription_stream(missing_stderr, &sender)
            .await
            .expect_err("missing stderr")
            .to_string()
            .contains("stderr is unavailable")
    );
}

#[tokio::test]
async fn converts_launch_failures_to_stream_errors() {
    let (sender, mut receiver) = channel();
    let options = SubscriptionOptions::internal(
        std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        std::time::Duration::from_secs(1),
    );
    run_subscription_stream(
        sender,
        "/definitely/missing/claude".into(),
        "model".to_owned(),
        "prompt".to_owned(),
        options,
    )
    .await;
    let output = output(&mut receiver).await;
    assert!(output.contains("event: error"));
    assert!(output.contains("failed to start Claude subscription"));
}

#[tokio::test]
async fn reports_invalid_json_from_a_process() {
    let (sender, _receiver) = channel();
    let error = consume_subscription_stream(child("printf 'not-json\\n'"), &sender)
        .await
        .expect_err("invalid process output");
    assert!(error.to_string().contains("invalid stream JSON"));
}
