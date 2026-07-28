use std::{convert::Infallible, process::Stdio, sync::Arc};

use axum::body::Bytes;
use serde_json::json;
use tokio::{process::Command, sync::mpsc};

use super::{
    SubscriptionStream, consume_subscription_stream, result_output_tokens, run_subscription_stream,
};
use crate::anthropic::{
    agent_effort::AgentEffortIntents,
    subscription::{SubscriptionOptions, SubscriptionToolContext},
    subscription_activity::SubscriptionActivity,
};

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
        text_closed: false,
        saw_tool_use: false,
        saw_result: false,
        next_index: 0,
        tools: Vec::new(),
        tool_context: None,
        activity: SubscriptionActivity::default(),
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
        text_closed: false,
        saw_tool_use: false,
        saw_result: false,
        next_index: 0,
        tools: Vec::new(),
        tool_context: None,
        activity: SubscriptionActivity::default(),
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
async fn empty_partial_delta_is_not_visible_output_and_remains_eligible_for_status() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        saw_result: false,
        next_index: 0,
        tools: Vec::new(),
        tool_context: None,
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            r#"{"type":"stream_event","event":{"delta":{"type":"text_delta","text":""}}}"#,
        )
        .await
        .expect("empty partial delta");
    assert!(output(&mut receiver).await.is_empty());
    assert!(!stream.text_started);

    stream
        .activity_keepalive(&sender)
        .await
        .expect("status after continued silence");
    assert!(
        output(&mut receiver)
            .await
            .contains("Claudex is still working")
    );
}

#[tokio::test]
async fn shows_activity_status_before_delayed_subscription_output() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        saw_result: false,
        next_index: 0,
        tools: Vec::new(),
        tool_context: None,
        activity: SubscriptionActivity::default(),
    };
    stream
        .activity_keepalive(&sender)
        .await
        .expect("visible activity status");
    stream
        .activity_keepalive(&sender)
        .await
        .expect("zero-width follow-up heartbeat");
    stream
        .handle_line(
            &sender,
            r#"{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"hello"}}}"#,
        )
        .await
        .expect("delayed text delta");

    let output = output(&mut receiver).await;
    assert!(output.contains("Claudex is still working; waiting for provider output"));
    assert!(output.contains("signature_delta"));
    assert_eq!(output.matches("event: content_block_start").count(), 2);
    let text_frame = output
        .split("\n\n")
        .find(|frame| frame.contains(r#""text":"hello""#))
        .expect("forwarded text frame");
    assert!(text_frame.contains(r#""index":1"#));
}

#[tokio::test]
async fn falls_back_to_result_text_and_estimated_tokens() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        saw_result: false,
        next_index: 0,
        tools: Vec::new(),
        tool_context: None,
        activity: SubscriptionActivity::default(),
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
        text_closed: false,
        saw_tool_use: false,
        saw_result: false,
        next_index: 0,
        tools: Vec::new(),
        tool_context: None,
        activity: SubscriptionActivity::default(),
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

#[tokio::test]
async fn collects_sequential_subscription_agents_into_one_outer_tool_round() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Task".to_owned()],
        tool_context: Some(explicit_subscription_tool_context()),
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant",
                "parent_tool_use_id":null,
                "message":{
                    "usage":{"output_tokens":7},
                    "content":[{
                        "type":"tool_use", "id":"tool-subscription", "name":"Agent",
                        "input":{"prompt":"work", "subagent_type":"claudex-gpt-spark", "claudex_model":"gpt-5.3-codex-spark"}
                    }]
                }
            })
            .to_string(),
        )
        .await
        .expect("forward subscription tool");
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"stream_event",
                "event":{"delta":{"type":"text_delta","text":"blocked inner tool"}}
            })
            .to_string(),
        )
        .await
        .expect("ignore text emitted after a forwarded tool");
    stream
        .activity_keepalive(&sender)
        .await
        .expect("show progress while collecting tools");
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant",
                "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"tool-subscription-2", "name":"Agent",
                    "input":{"prompt":"more work", "subagent_type":"claudex-grok", "claudex_model":"grok-4.5"}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("forward second subscription tool");
    stream
        .finish(
            &sender,
            &json!({"type":"result","subtype":"success","result":"done"}),
        )
        .await
        .expect("finish forwarded subscription tool");
    let output = output(&mut receiver).await;
    assert_eq!(output.matches(r#""name":"Task""#).count(), 2);
    assert!(output.contains(r#""name":"Task""#));
    assert!(output.contains(r#""input":{}"#));
    assert!(output.contains("input_json_delta"));
    assert!(output.contains("Claudex is still working"));
    assert!(output.contains("signature_delta"));
    assert!(!output.contains("blocked inner tool"));
    assert_eq!(output.matches(r#""stop_reason":"tool_use""#).count(), 1);
    assert!(output.contains(r#""stop_reason":"tool_use""#));
    assert!(output.find("input_json_delta") < output.find("content_block_stop"));
}

fn explicit_subscription_tool_context() -> SubscriptionToolContext {
    SubscriptionToolContext {
        agent_efforts: Arc::new(AgentEffortIntents::default()),
        client_user_id: None,
        parent_model: "parent-model".to_owned(),
        system: json!(null),
        user_messages: vec![json!({
            "role":"user", "content":"Use gpt-5.3-codex-spark and grok-4.5"
        })],
    }
}

#[tokio::test]
async fn sanitizes_and_records_contextual_agent_tool_input() {
    let (sender, mut receiver) = channel();
    let intents = Arc::new(AgentEffortIntents::default());
    let user_messages = vec![json!({
        "role":"user", "content":"Use gpt-test with high effort"
    })];
    let mut stream = SubscriptionStream {
        text_started: true,
        text_closed: false,
        saw_tool_use: false,
        saw_result: false,
        next_index: 1,
        tools: vec!["Agent".to_owned()],
        tool_context: Some(SubscriptionToolContext {
            agent_efforts: intents,
            client_user_id: Some("user".to_owned()),
            parent_model: "parent-model".to_owned(),
            system: json!([{"type":"text","text":"system message"}]),
            user_messages,
        }),
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"tool-context", "name":"Agent",
                    "input":{
                        "prompt":"work", "name":"invented",
                        "claudex_model":"gpt-test", "claudex_effort":"high"
                    }
                }]}
            })
            .to_string(),
        )
        .await
        .expect("forward contextual Agent tool");
    assert!(stream.text_closed);
    let output = output(&mut receiver).await;
    assert!(output.contains("<claudex-agent-id>tool-context</claudex-agent-id>"));
    assert!(!output.contains("claudex_model"));
    assert!(!output.contains("claudex_effort"));
    assert!(!output.contains("invented"));
}

#[tokio::test]
async fn rejects_each_malformed_subscription_tool_shape() {
    let (sender, _) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Agent".to_owned()],
        tool_context: None,
        activity: SubscriptionActivity::default(),
    };
    for block in [
        json!({"type":"tool_use", "name":"Agent", "input":{}}),
        json!({"type":"tool_use", "id":"missing-name", "input":{}}),
    ] {
        let error = stream
            .handle_line(
                &sender,
                &json!({
                    "type":"assistant", "parent_tool_use_id":null,
                    "message":{"content":[block]}
                })
                .to_string(),
            )
            .await
            .expect_err("missing tool identity must fail");
        assert!(error.to_string().contains("without an ID or name"));
    }
    let error = stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"invalid-input", "name":"Agent", "input":[]
                }]}
            })
            .to_string(),
        )
        .await
        .expect_err("non-object tool input must fail");
    assert!(error.to_string().contains("non-object tool input"));
}

#[tokio::test]
async fn ignores_non_top_level_tool_events_and_exercises_completed_state() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Read".to_owned(), "Agent".to_owned()],
        tool_context: Some(SubscriptionToolContext {
            agent_efforts: Arc::new(AgentEffortIntents::default()),
            client_user_id: None,
            parent_model: "parent-model".to_owned(),
            system: json!([{"type":"text","text":"system message"}]),
            user_messages: Vec::new(),
        }),
        activity: SubscriptionActivity::default(),
    };
    for envelope in [
        json!({"type":"assistant", "parent_tool_use_id":"parent"}),
        json!({"type":"assistant", "parent_tool_use_id":null}),
        json!({
            "type":"assistant", "parent_tool_use_id":null,
            "message":{"content":[{"type":"text", "text":"not a tool"}]}
        }),
    ] {
        stream
            .handle_line(&sender, &envelope.to_string())
            .await
            .unwrap();
    }

    assert_eq!(
        stream
            .prepare_tool_input("Read", "read", &json!({"path":"README.md"}))
            .expect("non-Agent input"),
        json!({"path":"README.md"})
    );
    assert!(
        stream
            .prepare_tool_input("Agent", "agent", &json!({"description":"missing prompt"}))
            .expect_err("missing Agent model")
            .to_string()
            .contains("missing required `claudex_model`")
    );
    stream.close_text(&sender).await.expect("close absent text");
    stream
        .finish(
            &sender,
            &json!({"type":"result", "subtype":"success", "result":"done"}),
        )
        .await
        .expect("first result");
    stream
        .finish(
            &sender,
            &json!({"type":"result", "subtype":"success", "result":"ignored"}),
        )
        .await
        .expect("duplicate result");
    stream
        .activity_keepalive(&sender)
        .await
        .expect("closed stream keepalive");
    stream.text_closed = false;
    stream
        .activity_keepalive(&sender)
        .await
        .expect("completed stream keepalive");
    assert!(output(&mut receiver).await.contains("done"));
}

#[tokio::test]
async fn accepts_a_valid_agent_model_without_a_prompt() {
    let (_sender, _receiver) = channel();
    let stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Agent".to_owned()],
        tool_context: Some(SubscriptionToolContext {
            agent_efforts: Arc::new(AgentEffortIntents::default()),
            client_user_id: None,
            parent_model: "parent-model".to_owned(),
            system: json!(null),
            user_messages: vec![json!({
                "role":"user",
                "content":"Use gpt-test for this worker"
            })],
        }),
        activity: SubscriptionActivity::default(),
    };
    assert_eq!(
        stream
            .prepare_tool_input(
                "Agent",
                "agent-no-prompt",
                &json!({"claudex_model":"gpt-test"})
            )
            .expect("model-only Agent input"),
        json!({"claudex_model":"gpt-test"})
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
async fn fast_subscription_result_skips_activity_status_and_requires_result_event() {
    let (sender, mut receiver) = channel();
    consume_subscription_stream(
        child(r#"printf '%s\n' '{"type":"result","subtype":"success","result":"done"}'"#),
        &sender,
    )
    .await
    .expect("successful subscription stream");
    let success = output(&mut receiver).await;
    assert!(success.contains("done"));
    assert!(!success.contains("Claudex is still working"));
    assert!(!success.contains(r#""type":"thinking""#));
    let text_frame = success
        .split("\n\n")
        .find(|frame| frame.contains(r#""text":"done""#))
        .expect("immediate text frame");
    assert!(text_frame.contains(r#""index":0"#));

    let (empty_sender, mut empty_receiver) = channel();
    consume_subscription_stream(
        child(r#"printf '%s\n' '{"type":"result","subtype":"success","result":""}'"#),
        &empty_sender,
    )
    .await
    .expect("successful empty subscription stream");
    let empty = output(&mut empty_receiver).await;
    assert!(empty.contains(r#""text":"""#));
    assert!(empty.contains("event: message_stop"));
    assert!(!empty.contains("Claudex is still working"));
    assert!(!empty.contains(r#""type":"thinking""#));

    let error =
        consume_subscription_stream(child("printf '%s\\n' '{\"type\":\"ignored\"}'"), &sender)
            .await
            .expect_err("missing result");
    assert!(error.to_string().contains("without a result"));
}

#[tokio::test]
async fn delayed_subscription_result_stays_quiet_under_activity_threshold() {
    // Initial activity delay is 30s (Claude-like quieter idle). A ~2s silent wait
    // must not inject a "still working" thinking block.
    let (sender, mut receiver) = channel();
    consume_subscription_stream(
        child(
            r#"sleep 2.1; printf '%s\n' '{"type":"result","subtype":"success","result":"done"}'"#,
        ),
        &sender,
    )
    .await
    .expect("delayed subscription stream");
    let frames = output(&mut receiver).await;
    assert!(!frames.contains("Claudex is still working"));
    assert!(frames.contains("done"));
}

#[tokio::test]
async fn early_text_is_not_split_by_the_initial_activity_deadline() {
    let (sender, mut receiver) = channel();
    let script = concat!(
        r#"printf '%s\n' '{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"ST"}}}'; "#,
        "sleep 2.1; ",
        r#"printf '%s\n' '{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"REAM_OK"}}}' "#,
        r#"'{"type":"result","subtype":"success","result":"STREAM_OK"}'"#,
    );
    consume_subscription_stream(child(script), &sender)
        .await
        .expect("stream with early text");
    let frames = output(&mut receiver).await;
    assert!(!frames.contains("Claudex is still working"));
    assert!(!frames.contains('\u{200b}'));
    assert!(frames.contains(r#""text":"ST""#));
    assert!(frames.contains(r#""text":"REAM_OK""#));
}

#[tokio::test]
async fn reports_process_failure_and_stderr() {
    let (sender, mut receiver) = channel();
    let error = consume_subscription_stream(child("printf 'fixture failure' >&2; exit 7"), &sender)
        .await
        .expect_err("failed process");
    let message = error.to_string();
    assert!(message.contains("fixture failure"));
    assert!(message.contains("exit status"));
    assert!(output(&mut receiver).await.is_empty());
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
