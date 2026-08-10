use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    fs,
    io::Cursor,
    process::Stdio,
    sync::Arc,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::{fs::PermissionsExt, process::CommandExt as _};
#[cfg(unix)]
use std::path::{Path, PathBuf};

use axum::body::Bytes;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt, BufReader},
    process::Command,
    sync::mpsc,
};

use super::{
    SubscriptionStream, consume_subscription_stream, consume_subscription_stream_with_options,
    result_output_tokens, run_subscription_stream, stream_subscription_model,
};
use crate::anthropic::{
    MessagesRequest,
    agent_effort::AgentEffortIntents,
    subagent_reuse::SubagentReuseRegistry,
    subscription::{SubscriptionOptions, SubscriptionToolContext},
    subscription_activity::SubscriptionActivity,
    subscription_stream::post_eof,
};
use crate::provider_config::{ModelCatalog, WorkerRoute};

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

#[derive(Default)]
struct StreamValidation {
    open_blocks: HashMap<u64, String>,
    started_blocks: HashSet<u64>,
    stopped_blocks: HashSet<u64>,
    stop_reasons: Vec<String>,
    message_stops: usize,
    terminal: bool,
}

fn parse_frame(raw_frame: &str) -> Value {
    let data = raw_frame
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("SSE data");
    serde_json::from_str(data).expect("SSE JSON")
}

impl StreamValidation {
    fn observe(&mut self, raw_frame: &str) {
        assert!(
            !self.terminal,
            "frame emitted after message_stop: {raw_frame}"
        );
        let frame = parse_frame(raw_frame);
        match frame["type"].as_str() {
            Some("content_block_start") => self.start_block(&frame),
            Some("content_block_delta") => self.validate_delta(&frame),
            Some("content_block_stop") => self.stop_block(&frame),
            Some("message_delta") => self.record_stop_reason(&frame),
            Some("message_stop") => self.stop_message(),
            _ => {}
        }
    }

    fn start_block(&mut self, frame: &Value) {
        let index = frame["index"].as_u64().expect("start index");
        assert_eq!(
            index,
            self.started_blocks.len() as u64,
            "content block indices must be contiguous and increasing"
        );
        let block_type = frame["content_block"]["type"]
            .as_str()
            .expect("content block type")
            .to_owned();
        assert!(
            self.started_blocks.insert(index),
            "duplicate block start: {index}"
        );
        assert!(self.open_blocks.insert(index, block_type).is_none());
    }

    fn validate_delta(&self, frame: &Value) {
        let index = frame["index"].as_u64().expect("delta index");
        let block_type = self
            .open_blocks
            .get(&index)
            .expect("delta targets open block");
        let delta_type = frame["delta"]["type"].as_str().expect("delta type");
        let expected_block_type = match delta_type {
            "text_delta" => "text",
            "input_json_delta" => "tool_use",
            "thinking_delta" | "signature_delta" => "thinking",
            other => panic!("unexpected delta type: {other}"),
        };
        assert_eq!(block_type, expected_block_type, "delta/block type mismatch");
    }

    fn stop_block(&mut self, frame: &Value) {
        let index = frame["index"].as_u64().expect("stop index");
        assert!(
            self.open_blocks.remove(&index).is_some(),
            "stop without open block: {index}"
        );
        assert!(
            self.stopped_blocks.insert(index),
            "duplicate block stop: {index}"
        );
    }

    fn record_stop_reason(&mut self, frame: &Value) {
        let Some(reason) = frame["delta"]["stop_reason"].as_str() else {
            return;
        };
        assert!(
            self.open_blocks.is_empty(),
            "terminal delta emitted before every block was closed"
        );
        self.stop_reasons.push(reason.to_owned());
    }

    fn stop_message(&mut self) {
        self.message_stops += 1;
        self.terminal = true;
    }

    fn assert_finished(self, expected_stop_reason: Option<&str>) {
        assert!(
            self.open_blocks.is_empty(),
            "unclosed blocks: {:?}",
            self.open_blocks
        );
        assert_eq!(
            self.started_blocks, self.stopped_blocks,
            "start/stop index mismatch"
        );
        match expected_stop_reason {
            Some(expected) => {
                assert_eq!(self.message_stops, 1, "message_stop must be unique");
                assert_eq!(self.stop_reasons, vec![expected.to_owned()]);
            }
            None => {
                assert_eq!(
                    self.message_stops, 0,
                    "failed stream must not emit message_stop"
                );
                assert!(self.stop_reasons.is_empty());
            }
        }
    }
}

fn assert_valid_stream(output: &str, expected_stop_reason: Option<&str>) {
    let mut validation = StreamValidation::default();
    for raw_frame in output.split("\n\n").filter(|frame| !frame.is_empty()) {
        validation.observe(raw_frame);
    }
    validation.assert_finished(expected_stop_reason);
}

fn collect_block_events(output: &str) -> (Vec<(u64, String)>, Vec<u64>) {
    let mut block_types = Vec::new();
    let mut stopped_indices = Vec::new();
    for raw_frame in output.split("\n\n").filter(|frame| !frame.is_empty()) {
        record_block_event(
            &parse_frame(raw_frame),
            &mut block_types,
            &mut stopped_indices,
        );
    }
    (block_types, stopped_indices)
}

fn record_block_event(
    frame: &Value,
    block_types: &mut Vec<(u64, String)>,
    stopped_indices: &mut Vec<u64>,
) {
    match frame["type"].as_str() {
        Some("content_block_start") => block_types.push((
            frame["index"].as_u64().expect("block index"),
            frame["content_block"]["type"]
                .as_str()
                .expect("block type")
                .to_owned(),
        )),
        Some("content_block_delta") if frame["delta"]["type"].as_str() == Some("text_delta") => {
            assert_text_delta_targets_text(frame, block_types);
        }
        Some("content_block_stop") => {
            stopped_indices.push(frame["index"].as_u64().expect("stop index"));
        }
        _ => {}
    }
}

fn assert_text_delta_targets_text(frame: &Value, block_types: &[(u64, String)]) {
    let index = frame["index"].as_u64().expect("text index");
    assert_eq!(
        block_types
            .iter()
            .find(|(block_index, _)| *block_index == index)
            .map(|(_, block_type)| block_type.as_str()),
        Some("text"),
        "text delta must target a text block: {frame}"
    );
}

#[tokio::test]
async fn handles_ignored_invalid_and_non_text_events() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
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
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
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
async fn keeps_native_web_results_inside_the_subscription() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["WebSearch".to_owned(), "WebFetch".to_owned()],
        tool_context: None,
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant",
                "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"web-search", "name":"WebSearch",
                    "input":{"query":"Example Robotics"}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("consume native web tool");
    stream
        .handle_line(
            &sender,
            r#"{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"https://example.invalid"}}}"#,
        )
        .await
        .expect("forward native web result");
    stream
        .handle_line(
            &sender,
            r#"{"type":"result","subtype":"success","result":"https://example.invalid"}"#,
        )
        .await
        .expect("finish native web result");
    let output = output(&mut receiver).await;
    assert_valid_stream(&output, Some("end_turn"));
    assert!(!output.contains("tool_use"));
    assert!(output.contains("https://example.invalid"));
    assert!(output.contains("end_turn"));
    assert!(!stream.saw_tool_use);
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"stream_event",
                "event":{"delta":{"type":"text_delta","text":"PRIVATE_PROVIDER_TAIL"}}
            })
            .to_string(),
        )
        .await
        .expect("provider text after a rejected launch is ignored");
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"second-unsupported", "name":"Agent",
                    "input":{"prompt":"duplicate", "subagent_type":"claudex-sonnet"}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("later provider tools after a rejected launch are ignored");
    assert!(stream.saw_result);
}

#[tokio::test]
async fn keeps_structured_output_internal_and_returns_its_json_result_as_text() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: Vec::new(),
        tool_context: None,
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant",
                "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use",
                    "id":"structured-output",
                    "name":"StructuredOutput",
                    "input":{"ok":true}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("consume internal structured output");
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"result",
                "subtype":"success",
                "result":"UNSTRUCTURED_FALLBACK_MUST_NOT_LEAK",
                "structured_output":{"ok":true}
            })
            .to_string(),
        )
        .await
        .expect("consume structured result");

    assert!(!stream.saw_tool_use);
    assert!(stream.saw_result);
    let output = output(&mut receiver).await;
    assert!(output.contains(r#"{\"ok\":true}"#));
    assert!(!output.contains("UNSTRUCTURED_FALLBACK_MUST_NOT_LEAK"));
    assert!(!output.contains("tool_use"));
}

#[tokio::test]
async fn empty_partial_delta_is_not_visible_output_and_remains_eligible_for_status() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
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
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
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
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
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
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
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
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
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
                        "input":{"prompt":"work", "subagent_type":"claudex-gpt-spark"}
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
        .expect("keepalive after tool_use must not reopen the turn");
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant",
                "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"tool-subscription-2", "name":"Agent",
                    "input":{"prompt":"more work", "subagent_type":"claudex-grok"}
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
    assert!(!output.contains("Claudex is still working"));
    assert!(!output.contains("signature_delta"));
    assert!(!output.contains("blocked inner tool"));
    assert_eq!(output.matches(r#""stop_reason":"tool_use""#).count(), 1);
    assert!(output.contains(r#""stop_reason":"tool_use""#));
    assert!(output.find("input_json_delta") < output.find("content_block_stop"));
}

#[tokio::test]
async fn consume_reader_forwards_three_sequential_agent_launches() {
    let (sender, mut receiver) = channel();
    let mut options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(2),
    );
    options.tools = vec!["Task".to_owned()];
    options.tool_context = Some(explicit_subscription_tool_context());
    let input = [
        json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"agent-1","name":"Agent","input":{"prompt":"scope a","subagent_type":"claudex-gpt-spark"}}]}}),
        json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"agent-2","name":"Agent","input":{"prompt":"scope b","subagent_type":"claudex-grok"}}]}}),
        json!({"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"agent-3","name":"Agent","input":{"prompt":"scope c","subagent_type":"claudex-gpt-spark"}}]}}),
        json!({"type":"result","subtype":"success","result":"done"}),
    ]
    .into_iter()
    .map(|value| value.to_string())
    .collect::<Vec<_>>()
    .join("\n");
    SubscriptionStream::consume_reader_for_test(
        Cursor::new(input),
        &sender,
        &options,
        "subscription-test",
    )
    .await
    .expect("three sequential Agent launches");
    let output = output(&mut receiver).await;
    assert_eq!(output.matches(r#""name":"Task""#).count(), 3);
}

#[tokio::test]
async fn launch_fanout_drain_ends_when_no_sibling_launch_arrives() {
    let (sender, mut receiver) = channel();
    let mut options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(2),
    );
    options.tools = vec!["Task".to_owned()];
    options.tool_context = Some(explicit_subscription_tool_context());
    let input = json!({
        "type":"assistant",
        "parent_tool_use_id":null,
        "message":{"content":[{"type":"tool_use","id":"agent-1","name":"Agent","input":{"prompt":"solo","subagent_type":"claudex-gpt-spark"}}]}
    })
    .to_string();
    let (mut writer, reader) = tokio::io::duplex(1024);
    tokio::spawn(async move {
        let _ = writer.write_all(format!("{input}\n").as_bytes()).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    SubscriptionStream::consume_reader_for_test(
        BufReader::new(reader),
        &sender,
        &options,
        "subscription-test",
    )
    .await
    .expect("fanout drain after a lone Agent launch");
    let output = output(&mut receiver).await;
    assert!(output.contains(r#""name":"Task""#));
    assert!(output.contains(r#""stop_reason":"tool_use""#) || output.contains("end_turn"));
}

#[tokio::test]
async fn consume_iteration_emits_keepalive_when_activity_deadline_elapses() {
    let (sender, _receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: Vec::new(),
        tool_context: None,
        activity: SubscriptionActivity::default(),
    };
    let (_writer, reader) = tokio::io::duplex(64);
    let mut lines = BufReader::new(reader).lines();
    let mut pending = None;
    let mut deadline = Box::pin(tokio::time::sleep(Duration::from_millis(5)));
    let iteration = super::consume_fanout::consume_stream_iteration(
        &mut lines,
        &sender,
        "claude-sonnet-5",
        &mut stream,
        &mut pending,
        &mut deadline,
        Duration::from_millis(50),
    )
    .await
    .expect("activity deadline keepalive");
    assert!(matches!(
        iteration,
        super::consume_fanout::StreamIteration::Continue
    ));
}

#[tokio::test]
async fn consume_iteration_hides_lines_after_a_pending_result() {
    let (sender, _receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: Vec::new(),
        tool_context: None,
        activity: SubscriptionActivity::default(),
    };
    let (mut writer, reader) = tokio::io::duplex(256);
    writer
        .write_all(b"{\"type\":\"assistant\"}\n")
        .await
        .expect("write pending-result line");
    let mut lines = BufReader::new(reader).lines();
    let mut pending = Some(json!({"type": "result"}));
    let mut deadline = Box::pin(tokio::time::sleep(Duration::from_secs(30)));
    let iteration = super::consume_fanout::consume_stream_iteration(
        &mut lines,
        &sender,
        "claude-sonnet-5",
        &mut stream,
        &mut pending,
        &mut deadline,
        Duration::from_secs(30),
    )
    .await
    .expect("hidden pending result");
    assert!(matches!(
        iteration,
        super::consume_fanout::StreamIteration::Continue
    ));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn blocked_agent_after_a_forwarded_tool_uses_a_fresh_text_block() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Task".to_owned()],
        tool_context: Some(routed_subscription_tool_context()),
        activity: SubscriptionActivity::default(),
    };

    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant",
                "parent_tool_use_id":null,
                "message":{
                    "content":[
                        {
                            "type":"tool_use", "id":"supported-agent", "name":"Agent",
                            "input":{
                                "description":"first worker", "prompt":"do first work",
                                "subagent_type":"claudex-gpt-spark",
                                "claudex_model":"gpt-5.3-codex-spark"
                            }
                        },
                        {
                            "type":"tool_use", "id":"blocked-agent", "name":"Agent",
                            "input":{
                                "description":"second worker", "prompt":"do second work",
                                "subagent_type":"claude-sonnet-5",
                                "claudex_model":"claude-sonnet-5"
                            }
                        },
                        {
                            "type":"tool_use", "id":"blocked-agent-2", "name":"Agent",
                            "input":{
                                "description":"third worker", "prompt":"do third work",
                                "subagent_type":"claude-fable",
                                "claudex_model":"claude-fable"
                            }
                        }
                    ]
                }
            })
            .to_string(),
        )
        .await
        .expect("forward mixed tool calls");
    stream
        .finish(
            &sender,
            &json!({"type":"result","subtype":"success","result":"done"}),
        )
        .await
        .expect("finish mixed tool calls");

    let output = output(&mut receiver).await;
    assert_valid_stream(&output, Some("tool_use"));
    let (block_types, stopped_indices) = collect_block_events(&output);
    assert_eq!(
        block_types,
        vec![(0, "tool_use".to_owned()), (1, "text".to_owned())]
    );
    for index in [0, 1] {
        assert_eq!(
            stopped_indices
                .iter()
                .filter(|stopped| **stopped == index)
                .count(),
            1,
            "block {index} must be closed exactly once"
        );
    }
    assert!(output.contains("The requested SubAgent model is not configured"));
    assert_eq!(output.matches(r#""stop_reason":"tool_use""#).count(), 1);
}

#[tokio::test]
async fn resumes_text_on_a_fresh_index_after_internal_web_search() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["WebSearch".to_owned()],
        tool_context: None,
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            r#"{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"before"}}}"#,
        )
        .await
        .expect("first text");
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"synthetic-search", "name":"WebSearch",
                    "input":{"query":"Example Robotics"}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("internal search");
    stream
        .handle_line(
            &sender,
            r#"{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"after"}}}"#,
        )
        .await
        .expect("second text");
    stream
        .finish(
            &sender,
            &json!({"type":"result","subtype":"success","result":"done"}),
        )
        .await
        .expect("finish text stream");

    let output = output(&mut receiver).await;
    assert_valid_stream(&output, Some("end_turn"));
    assert!(output.contains(r#""index":0"#));
    assert!(output.contains(r#""index":1"#));
}

#[tokio::test]
async fn deduplicates_replayed_tool_ids_and_preserves_tool_terminal_state() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Agent".to_owned()],
        tool_context: Some(explicit_subscription_tool_context()),
        activity: SubscriptionActivity::default(),
    };
    let envelope = json!({
        "type":"assistant", "parent_tool_use_id":null,
        "message":{"content":[{
            "type":"tool_use", "id":"synthetic-replayed-tool", "name":"Agent",
            "input":{
                "prompt":"bounded synthetic work", "subagent_type":"claudex-gpt-spark",
                "claudex_model":"gpt-5.3-codex-spark"
            }
        }]}
    })
    .to_string();
    stream
        .handle_line(&sender, &envelope)
        .await
        .expect("first tool");
    stream
        .handle_line(&sender, &envelope)
        .await
        .expect("replayed tool");
    stream
        .finish(
            &sender,
            &json!({"type":"result","subtype":"success","result":"done"}),
        )
        .await
        .expect("finish tool stream");

    let output = output(&mut receiver).await;
    assert_valid_stream(&output, Some("tool_use"));
    let forwarded_tool_starts = output
        .split("\n\n")
        .filter_map(|raw_frame| {
            raw_frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
        })
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .filter(|frame| {
            frame["type"].as_str() == Some("content_block_start")
                && frame["content_block"]["type"].as_str() == Some("tool_use")
                && frame["content_block"]["id"].as_str() == Some("synthetic-replayed-tool")
        })
        .count();
    assert_eq!(forwarded_tool_starts, 1);
}

fn explicit_subscription_tool_context() -> SubscriptionToolContext {
    SubscriptionToolContext::for_tests(
        Arc::new(AgentEffortIntents::default()),
        ModelCatalog::default(),
        None,
        "parent-model",
        vec![json!({
            "role":"user", "content":"Claudex routing for this turn: {\"providers\":{},\"selected_workers\":[{\"agent\":\"claudex-gpt-spark\",\"model\":\"gpt-5.3-codex-spark\",\"effort\":\"xhigh\"},{\"agent\":\"claudex-grok\",\"model\":\"grok-4.5\",\"effort\":\"high\"}]}"
        })],
        json!(null),
    )
}

fn routed_subscription_tool_context() -> SubscriptionToolContext {
    let mut context = explicit_subscription_tool_context();
    context.user_messages = vec![
        json!({
            "role":"assistant",
            "content":[{"type":"tool_use","name":"Agent","id":"prior-agent","input":{}}]
        }),
        context.user_messages[0].clone(),
        json!({"role":"user", "content":"launch the selected worker now"}),
    ];
    context
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
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 1,
        tools: vec!["Agent".to_owned()],
        tool_context: Some(SubscriptionToolContext::for_tests(
            intents,
            ModelCatalog::default(),
            Some("user".to_owned()),
            "parent-model",
            user_messages,
            json!([{"type":"text","text":"system message"}]),
        )),
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
    assert!(output.contains("claudex_launch_id: tool-context"));
    assert!(output.contains("claudex_model: gpt-test"));
    assert!(!output.contains(r#"\"claudex_model\":"#));
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
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
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
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Read".to_owned(), "Agent".to_owned()],
        tool_context: Some(SubscriptionToolContext::for_tests(
            Arc::new(AgentEffortIntents::default()),
            ModelCatalog::default(),
            None,
            "parent-model",
            Vec::new(),
            json!([{"type":"text","text":"system message"}]),
        )),
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
            .prepare_tool_input(
                "Agent",
                "agent",
                &json!({"subagent_type":"custom-worker", "description":"missing prompt"})
            )
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
async fn blocks_an_unsupported_subagent_without_failing_the_parent_stream() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Agent".to_owned()],
        tool_context: Some(SubscriptionToolContext::for_tests(
            Arc::new(AgentEffortIntents::default()),
            ModelCatalog::default(),
            None,
            "claude-haiku-4-5",
            Vec::new(),
            json!(null),
        )),
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"unsupported", "name":"Agent",
                    "input":{"prompt":"work", "subagent_type":"claudex-sonnet"}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("unsupported SubAgent must not fail the parent stream");
    assert!(!stream.saw_tool_use);
    stream
        .finish(
            &sender,
            &json!({"type":"result", "subtype":"success", "result":""}),
        )
        .await
        .expect("finish parent stream");
    let output = output(&mut receiver).await;
    assert!(output.contains("was not started. Continue without it."));
    assert_eq!(
        output
            .matches("was not started. Continue without it.")
            .count(),
        1
    );
    assert!(!output.contains("PRIVATE_PROVIDER_TAIL"));
    assert!(!output.contains("second-unsupported"));
    assert_valid_stream(&output, Some("end_turn"));
    assert!(output.contains(r#""stop_reason":"end_turn""#));
    assert!(!output.contains(r#""stop_reason":"tool_use""#));
    assert!(!output.contains(r#""type":"tool_use""#));
}

#[tokio::test]
async fn blocks_exhausted_ollama_glm_launch_without_emitting_tool_use() {
    use std::time::SystemTime;

    use crate::anthropic::provider_auth_cooldown;

    let root = tempfile::tempdir().expect("ollama cooldown fixture");
    let cache = provider_auth_cooldown::cache_path_for_home(root.path());
    assert!(
        provider_auth_cooldown::record_rate_limit_at(
            Some(&cache),
            "ollama",
            "429 Too Many Requests",
            SystemTime::now(),
        )
        .is_some()
    );
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![
            WorkerRoute::new("claudex-ollama-glm-5-2", "glm-5.2:cloud", "max")
                .with_usage_provider(Some("ollama".to_owned())),
            WorkerRoute::new("claudex-gpt-spark", "gpt-5.3-codex-spark", "xhigh"),
        ])
        .expect("install workers");
    let mut context = SubscriptionToolContext::for_tests(
        Arc::new(AgentEffortIntents::default()),
        catalog,
        None,
        "claude-opus-5",
        vec![json!({
            "role":"user",
            "content":"Claudex routing for this turn: {\"providers\":{},\"selected_workers\":[{\"agent\":\"claudex-ollama-glm-5-2\",\"model\":\"glm-5.2:cloud\",\"effort\":\"max\"},{\"agent\":\"claudex-gpt-spark\",\"model\":\"gpt-5.3-codex-spark\",\"effort\":\"xhigh\"}],\"disabled_subagent_models\":[]}"
        })],
        json!(null),
    );
    context.auth_cache = Some(cache);
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Agent".to_owned()],
        tool_context: Some(context),
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"ollama-limit", "name":"Agent",
                    "input":{"prompt":"work", "subagent_type":"claudex-ollama-glm-5-2"}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("exhausted ollama launch must not fail the parent stream");
    assert!(!stream.saw_tool_use);
    stream
        .finish(
            &sender,
            &json!({"type":"result", "subtype":"success", "result":""}),
        )
        .await
        .expect("finish parent stream");
    let output = output(&mut receiver).await;
    assert!(
        output.contains("glm-5.2:cloud") && output.contains("cooling down"),
        "{output}"
    );
    assert!(!output.contains(r#""type":"tool_use""#));
    assert_valid_stream(&output, Some("end_turn"));
}

#[tokio::test]
async fn blocks_routing_disabled_ollama_glm_without_cooldown_file() {
    let mut catalog = ModelCatalog::default();
    catalog
        .set_worker_routes(vec![WorkerRoute::new(
            "claudex-ollama-glm-5-2",
            "glm-5.2:cloud",
            "max",
        )])
        .expect("install glm worker");
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Agent".to_owned()],
        tool_context: Some(SubscriptionToolContext::for_tests(
            Arc::new(AgentEffortIntents::default()),
            catalog,
            None,
            "claude-opus-5",
            vec![json!({
                "role":"user",
                "content":"Claudex routing for this turn: {\"providers\":{\"ollama-glm\":{\"available\":false,\"reason\":\"exhausted\",\"model\":\"glm-5.2:cloud\"}},\"selected_workers\":[{\"agent\":\"claudex-gpt-spark\",\"model\":\"gpt-5.3-codex-spark\",\"effort\":\"xhigh\"}],\"disabled_subagent_models\":[\"glm-5.2:cloud\"]}"
            })],
            json!(null),
        )),
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"ollama-weekly-limit", "name":"Agent",
                    "input":{"prompt":"work", "subagent_type":"claudex-ollama-glm-5-2"}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("CodexBar-disabled glm must not fail the parent stream");
    assert!(!stream.saw_tool_use);
    stream
        .finish(
            &sender,
            &json!({"type":"result", "subtype":"success", "result":""}),
        )
        .await
        .expect("finish parent stream");
    let output = output(&mut receiver).await;
    assert!(
        output.contains("glm-5.2:cloud") && output.contains("cooling down"),
        "{output}"
    );
    assert!(!output.contains(r#""type":"tool_use""#));
}

#[tokio::test]
async fn skips_task_stop_for_background_shell_or_foreign_ids() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["TaskStop".to_owned(), "Stop Task".to_owned()],
        tool_context: None,
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"stop-foreign", "name":"TaskStop",
                    "input":{"task_id":"b13mjnjlj"}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("skip foreign TaskStop");
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"stop-shell", "name":"Stop Task",
                    "input":{"task_id":"bjh859kgm"}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("skip bash-background TaskStop");
    stream
        .finish(
            &sender,
            &json!({"type":"result","subtype":"success","result":"done"}),
        )
        .await
        .expect("finish skipped TaskStop");
    assert!(!stream.saw_tool_use);
    let output = output(&mut receiver).await;
    assert_valid_stream(&output, Some("end_turn"));
    assert!(!output.contains(r#""type":"tool_use""#));
    assert!(!output.contains("No task found"));
    assert!(output.contains("b13mjnjlj"));
    assert!(output.contains("TaskStop skipped"));
    assert!(output.contains(r#""stop_reason":"end_turn""#));
}

#[tokio::test]
async fn forwards_task_stop_for_live_claude_code_agent_ids() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["TaskStop".to_owned()],
        tool_context: None,
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"stop-agent", "name":"TaskStop",
                    "input":{"task_id":"a4b2412c427ee5327"}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("forward live Agent TaskStop");
    stream
        .finish(
            &sender,
            &json!({"type":"result","subtype":"success","result":"done"}),
        )
        .await
        .expect("finish live Agent TaskStop");
    assert!(stream.saw_tool_use);
    let output = output(&mut receiver).await;
    assert_valid_stream(&output, Some("tool_use"));
    assert!(output.contains(r#""type":"tool_use""#));
    assert!(output.contains("a4b2412c427ee5327"));
    assert!(output.contains(r#""stop_reason":"tool_use""#));
    assert!(!output.contains("TaskStop skipped"));
}

#[tokio::test]
async fn skips_stale_task_output_when_live_agents_exist() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["TaskOutput".to_owned()],
        tool_context: Some(SubscriptionToolContext::for_tests(
            Arc::new(AgentEffortIntents::default()),
            ModelCatalog::default(),
            None,
            "parent-model",
            vec![json!({
                "role":"user",
                "content":[{
                    "type":"tool_result",
                    "tool_use_id":"toolu_live",
                    "content":[{"type":"text","text":"Async agent launched successfully.\nagentId: a4496564387a2561f"}]
                }]
            })],
            json!(null),
        )),
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"stale-output", "name":"TaskOutput",
                    "input":{"task_id":"a3d7f2ca50556c9e5","block":false}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("skip stale TaskOutput");
    stream
        .finish(
            &sender,
            &json!({"type":"result","subtype":"success","result":"done"}),
        )
        .await
        .expect("finish skipped TaskOutput");
    assert!(!stream.saw_tool_use);
    let output = output(&mut receiver).await;
    assert_valid_stream(&output, Some("end_turn"));
    assert!(!output.contains(r#""type":"tool_use""#));
    assert!(output.contains("a3d7f2ca50556c9e5"));
    assert!(output.contains("a4496564387a2561f"));
    assert!(output.contains("TaskOutput skipped"));
    assert!(output.contains(r#""stop_reason":"end_turn""#));
}

#[tokio::test]
async fn forwards_task_output_for_live_claude_code_agent_ids() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["TaskOutput".to_owned()],
        tool_context: Some(SubscriptionToolContext::for_tests(
            Arc::new(AgentEffortIntents::default()),
            ModelCatalog::default(),
            None,
            "parent-model",
            vec![json!({
                "role":"user",
                "content":[{
                    "type":"tool_result",
                    "tool_use_id":"toolu_live",
                    "content":[{"type":"text","text":"Async agent launched successfully.\nagentId: a4496564387a2561f"}]
                }]
            })],
            json!(null),
        )),
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"live-output", "name":"TaskOutput",
                    "input":{"task_id":"a4496564387a2561f","block":false}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("forward live TaskOutput");
    stream
        .finish(
            &sender,
            &json!({"type":"result","subtype":"success","result":"done"}),
        )
        .await
        .expect("finish live TaskOutput");
    assert!(stream.saw_tool_use);
    let output = output(&mut receiver).await;
    assert_valid_stream(&output, Some("tool_use"));
    assert!(output.contains(r#""type":"tool_use""#));
    assert!(output.contains("a4496564387a2561f"));
    assert!(!output.contains("TaskOutput skipped"));
    assert!(output.contains(r#""stop_reason":"tool_use""#));
}

#[tokio::test]
async fn accepts_a_valid_agent_model_without_a_prompt() {
    let (_sender, _receiver) = channel();
    let stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Agent".to_owned()],
        tool_context: Some(SubscriptionToolContext::for_tests(
            Arc::new(AgentEffortIntents::default()),
            ModelCatalog::default(),
            None,
            "parent-model",
            vec![json!({
                "role":"user",
                "content":"Use gpt-test for this worker"
            })],
            json!(null),
        )),
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
        json!({"run_in_background": true})
    );
}

#[tokio::test]
async fn prepare_tool_input_rewrites_same_scope_launch_to_resume() {
    let registry = Arc::new(SubagentReuseRegistry::default());
    let mut recorded = MessagesRequest {
        model: "main".to_owned(),
        system: json!("stable system"),
        messages: vec![
            json!({
                "role":"assistant",
                "content":[{
                    "type":"tool_use",
                    "id":"tool-a",
                    "name":"Agent",
                    "input":{
                        "prompt":"Audit the Rust adapter tests",
                        "claudex_model":"gpt-test"
                    }
                }]
            }),
            json!({
                "role":"user",
                "content":[{
                    "type":"tool_result",
                    "tool_use_id":"tool-a",
                    "content":[{"type":"text","text":"Async agent launched successfully.\\nagentId: worker-a"}]
                }]
            }),
        ],
        tools: vec![json!({"name":"Agent"})],
        stream: false,
        output_config: Value::Null,
        metadata: json!({"_claudex_transport_identity":{"session_id":"session-a"}}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };
    registry.observe_and_restore(&mut recorded);

    let mut context = SubscriptionToolContext::for_tests(
        Arc::new(AgentEffortIntents::default()),
        ModelCatalog::default(),
        None,
        "parent-model",
        vec![json!({"role":"user","content":"Use gpt-test for this worker"})],
        json!(null),
    );
    context.session_id = Some("session-a".to_owned());
    context.subagent_reuse = Arc::clone(&registry);
    let stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Agent".to_owned()],
        tool_context: Some(context),
        activity: SubscriptionActivity::default(),
    };
    let public = stream
        .prepare_tool_input(
            "Agent",
            "agent-reuse",
            &json!({
                "prompt":"Audit the Rust adapter tests",
                "claudex_model":"gpt-test"
            }),
        )
        .expect("same-scope launch should resume");
    assert_eq!(public["resume"], "worker-a");
    assert_eq!(public["run_in_background"], true);
}

#[tokio::test]
async fn same_turn_duplicate_scope_forwards_only_one_agent() {
    let (sender, mut receiver) = channel();
    let registry = Arc::new(SubagentReuseRegistry::default());
    let mut context = SubscriptionToolContext::for_tests(
        Arc::new(AgentEffortIntents::default()),
        ModelCatalog::default(),
        None,
        "parent-model",
        vec![json!({"role":"user","content":"Use gpt-test for this worker"})],
        json!(null),
    );
    context.session_id = Some("session-a".to_owned());
    context.subagent_reuse = Arc::clone(&registry);
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Agent".to_owned()],
        tool_context: Some(context),
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[
                    {
                        "type":"tool_use", "id":"reproduce-gpt", "name":"Agent",
                        "input":{
                            "description":"Reproduce azookey conversion bug",
                            "prompt":"Use gpt to reproduce はしのはじから.",
                            "claudex_model":"gpt-test"
                        }
                    },
                    {
                        "type":"tool_use", "id":"reproduce-cc", "name":"Agent",
                        "input":{
                            "description":"Reproduce azookey conversion bug",
                            "prompt":"Use another provider to reproduce はしのはじから.",
                            "claudex_model":"gpt-test"
                        }
                    },
                    {
                        "type":"tool_use", "id":"trace-cursor", "name":"Agent",
                        "input":{
                            "description":"Trace azookey conversion pipeline",
                            "prompt":"Map Vibrato boundaries across three surfaces.",
                            "claudex_model":"gpt-test"
                        }
                    }
                ]}
            })
            .to_string(),
        )
        .await
        .expect("forward independent scopes only");
    drop(sender);
    let output = output(&mut receiver).await;
    assert!(
        output.contains("reproduce-gpt"),
        "first same-scope launch must forward: {output}"
    );
    assert!(
        !output.contains("reproduce-cc"),
        "duplicate same-scope launch must not spawn another worker: {output}"
    );
    assert!(
        output.contains("trace-cursor"),
        "independent scope must still forward: {output}"
    );
    assert!(registry.scope_is_occupied(
        "session-a",
        &json!({"description":"Reproduce azookey conversion bug"})
    ));
}

#[tokio::test]
async fn routes_a_standard_general_purpose_agent_to_a_claudex_worker() {
    let (_sender, _receiver) = channel();
    let mut model_catalog = ModelCatalog::default();
    model_catalog
        .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
            "claudex-worker".to_owned(),
            "worker-model".to_owned(),
            "max".to_owned(),
        )])
        .expect("valid worker route");
    let stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Agent".to_owned()],
        tool_context: Some(SubscriptionToolContext::for_tests(
            Arc::new(AgentEffortIntents::default()),
            model_catalog,
            None,
            "claude-sonnet-5",
            Vec::new(),
            json!(null),
        )),
        activity: SubscriptionActivity::default(),
    };
    let routed = stream
        .prepare_tool_input(
            "Agent",
            "agent-standard",
            &json!({"prompt":"work", "subagent_type":"general-purpose"}),
        )
        .expect("standard Agent input");
    let prompt = routed["prompt"].as_str().expect("correlated prompt");
    assert!(prompt.contains("claudex_model: worker-model"));
    assert!(prompt.contains("<claudex-agent-id>agent-standard</claudex-agent-id>"));
    assert_eq!(routed["subagent_type"], "general-purpose");
    assert!(routed.get("claudex_model").is_none());
    let context = stream.tool_context.as_ref().expect("routing context");
    let pending = context
        .agent_efforts
        .pending
        .lock()
        .expect("agent effort intents lock");
    let intent = pending.back().expect("recorded worker intent");
    assert_eq!(intent.model_override.as_deref(), Some("worker-model"));
    assert_eq!(intent.effort.as_deref(), Some("max"));
    assert!(!intent.model_is_inherited);
}

fn child(script: &str) -> tokio::process::Child {
    let mut command = Command::new("sh");
    #[cfg(unix)]
    command.as_std_mut().process_group(0);
    command
        .args(["-c", script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn stream fixture")
}

#[cfg(unix)]
struct BackgroundSleepFixture {
    _directory: tempfile::TempDir,
    pid_file: PathBuf,
    release_file: PathBuf,
}

#[cfg(unix)]
impl BackgroundSleepFixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("create background process fixture directory");
        let pid_file = directory.path().join("background.pid");
        let release_file = directory.path().join("release");
        Self {
            _directory: directory,
            pid_file,
            release_file,
        }
    }

    fn script(&self, after_release: &str) -> String {
        format!(
            r#"set -eu
sleep 30 &
background_pid=$!
printf '%s\n' "$background_pid" > '{pid_file}.tmp'
mv '{pid_file}.tmp' '{pid_file}'
while [ ! -f '{release_file}' ]; do
    sleep 0.01
done
{after_release}
wait "$background_pid"
"#,
            pid_file = self.pid_file.display(),
            release_file = self.release_file.display(),
        )
    }

    fn stderr_holder_script(&self, after_release: &str) -> String {
        format!(
            r#"set -eu
sleep 30 >/dev/null &
background_pid=$!
printf '%s\n' "$background_pid" > '{pid_file}.tmp'
mv '{pid_file}.tmp' '{pid_file}'
while [ ! -f '{release_file}' ]; do
    sleep 0.01
done
{after_release}
wait "$background_pid"
"#,
            pid_file = self.pid_file.display(),
            release_file = self.release_file.display(),
        )
    }

    fn program(&self) -> PathBuf {
        let program = self._directory.path().join("stalled-stream.sh");
        fs::write(&program, format!("#!/bin/sh\n{}", self.script(":")))
            .expect("write stalled stream fixture");
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
            .expect("make stalled stream fixture executable");
        program
    }

    async fn release_after_pid(&self) -> Option<BackgroundProcessGuard> {
        let guard = wait_for_background_process(&self.pid_file).await?;
        fs::write(&self.release_file, b"release").expect("release background process fixture");
        Some(guard)
    }
}

#[cfg(unix)]
struct BackgroundProcessGuard {
    pid: libc::pid_t,
    active: bool,
}

#[cfg(unix)]
impl BackgroundProcessGuard {
    async fn assert_exited(&mut self) {
        let exited = wait_for_process_exit(self.pid)
            .await
            .expect("query background process");
        assert!(
            exited,
            "background process {} survived process-group termination",
            self.pid
        );
        self.active = false;
    }
}

#[cfg(unix)]
impl Drop for BackgroundProcessGuard {
    fn drop(&mut self) {
        if self.active {
            // SAFETY: The test owns this child PID and only sends SIGKILL as cleanup.
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(unix)]
fn process_is_gone(pid: libc::pid_t) -> std::io::Result<bool> {
    // SAFETY: Signal zero only queries a process ID and has no process side effect.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return Ok(false);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(true);
    }
    Err(error)
}

#[cfg(unix)]
async fn wait_for_process_exit(pid: libc::pid_t) -> std::io::Result<bool> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        if process_is_gone(pid)? {
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(false)
}

#[cfg(unix)]
async fn wait_for_background_process(pid_file: &Path) -> Option<BackgroundProcessGuard> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        if let Ok(raw_pid) = fs::read_to_string(pid_file)
            && let Ok(pid) = raw_pid.trim().parse::<libc::pid_t>()
            && pid > 1
        {
            return Some(BackgroundProcessGuard { pid, active: true });
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    None
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
async fn keepalive_after_forwarded_tool_use_does_not_inject_still_working() {
    let (sender, mut receiver) = channel();
    let mut stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Bash".to_owned()],
        tool_context: None,
        activity: SubscriptionActivity::default(),
    };
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant",
                "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"bash-1", "name":"Bash",
                    "input":{"command":"true"}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("forward Bash");
    stream
        .activity_keepalive(&sender)
        .await
        .expect("silent keepalive after tool_use");
    stream
        .finish(
            &sender,
            &json!({"type":"result","subtype":"success","result":"done"}),
        )
        .await
        .expect("finish Bash tool_use");
    let frames = output(&mut receiver).await;
    assert_valid_stream(&frames, Some("tool_use"));
    assert!(!frames.contains("Claudex is still working"));
    assert!(!frames.contains('\u{200b}'));
}

#[tokio::test]
async fn forwarded_tool_use_releases_the_client_before_subscription_result() {
    let (sender, mut receiver) = channel();
    let mut options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(8),
    );
    options.tools = vec!["Bash".to_owned()];
    options.initial_activity_delay = Duration::from_millis(30);
    options.activity_keepalive_interval = Duration::from_millis(30);
    let started = tokio::time::Instant::now();
    consume_subscription_stream_with_options(
        &mut child(
            r#"printf '%s\n' '{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"bash-1","name":"Bash","input":{"command":"true"}}]}}'; sleep 2.5; printf '%s\n' '{"type":"result","subtype":"success","result":"late"}'"#,
        ),
        &sender,
        &options,
        "subscription-test",
    )
    .await
    .expect("tool_use must end the SSE turn without waiting for subscription result");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "client stayed queued for {elapsed:?} after tool_use"
    );
    let frames = output(&mut receiver).await;
    assert_valid_stream(&frames, Some("tool_use"));
    assert!(frames.contains(r#""name":"Bash""#));
    assert!(frames.contains("bash-1"));
    assert!(!frames.contains("Claudex is still working"));
    assert!(!frames.contains(r#""text":"late""#));
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
async fn hidden_provider_events_do_not_starve_client_visible_activity() {
    let (sender, mut receiver) = channel();
    let mut options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(1),
    );
    options.is_subagent = true;
    options.effort = Some("high".to_owned());
    options.initial_activity_delay = Duration::ZERO;
    options.activity_keepalive_interval = Duration::from_secs(60);
    let input = [
        r#"{"type":"system","subtype":"init"}"#,
        r#"{"type":"system","subtype":"init"}"#,
        r#"{"type":"result","subtype":"success","result":"done"}"#,
    ]
    .join("\n");
    let reader = BufReader::new(Cursor::new(input.into_bytes()));
    SubscriptionStream::consume_reader_for_test(reader, &sender, &options, "subscription-test")
        .await
        .expect("ready hidden events must retain client-visible activity");
    let frames = output(&mut receiver).await;
    assert_eq!(
        frames
            .matches("SubAgent starting: subscription-test")
            .count(),
        1
    );
    assert!(frames.contains("effort=high"));
    assert!(frames.contains("done"));
    assert_valid_stream(&frames, Some("end_turn"));
    let (block_types, stopped_indices) = collect_block_events(&frames);
    assert_eq!(
        block_types,
        vec![(0, "thinking".to_owned()), (1, "text".to_owned())]
    );
    assert_eq!(stopped_indices, vec![0, 1]);
}

fn short_post_eof_options() -> SubscriptionOptions {
    let mut options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(1),
    );
    options.initial_activity_delay = Duration::from_millis(5);
    options.activity_keepalive_interval = Duration::from_millis(15);
    options.stderr_drain_grace = Duration::from_millis(40);
    options.termination_timeout = Duration::from_millis(500);
    options
}

#[tokio::test]
async fn post_eof_stderr_helpers_cover_empty_success_error_and_timeout() {
    let mut absent = None;
    assert_eq!(
        post_eof::reap_stderr(&mut absent, Duration::from_millis(1))
            .await
            .expect("absent stderr task"),
        Vec::<u8>::new()
    );

    let mut successful = Some(tokio::spawn(async {
        Ok::<Vec<u8>, std::io::Error>(b"stderr".to_vec())
    }));
    assert_eq!(
        post_eof::reap_stderr(&mut successful, Duration::from_millis(100),)
            .await
            .expect("successful stderr task"),
        b"stderr"
    );

    let mut failed = Some(tokio::spawn(async {
        Err::<Vec<u8>, _>(std::io::Error::other("stderr failed"))
    }));
    assert!(
        post_eof::reap_stderr(&mut failed, Duration::from_millis(100),)
            .await
            .expect_err("stderr task error")
            .to_string()
            .contains("stderr failed")
    );

    let mut timed_out = Some(tokio::spawn(async {
        std::future::pending::<std::io::Result<Vec<u8>>>().await
    }));
    assert_eq!(
        post_eof::reap_stderr(&mut timed_out, Duration::from_millis(1),)
            .await
            .expect("timed out stderr task is discarded"),
        Vec::<u8>::new()
    );

    let mut no_task = None;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(1),
            post_eof::await_stderr(&mut no_task),
        )
        .await
        .is_err()
    );

    let mut task = Some(tokio::spawn(async {
        Ok::<Vec<u8>, std::io::Error>(b"taken".to_vec())
    }));
    let taken = task.take().expect("take task").await;
    let mut task = Some(tokio::spawn(async {
        Ok::<Vec<u8>, std::io::Error>(b"taken".to_vec())
    }));
    assert_eq!(
        post_eof::take_stderr(&mut task, taken).expect("take stderr output"),
        b"taken"
    );
}

#[tokio::test]
async fn stdout_eof_keeps_activity_visible_until_the_leader_exits() {
    let (sender, mut receiver) = channel();
    let options = short_post_eof_options();
    let mut process = child(concat!(
        r#"printf '%s\n' '{"type":"result","subtype":"success","result":"done"}'; "#,
        "exec 1>&-; sleep 0.08",
    ));
    consume_subscription_stream_with_options(&mut process, &sender, &options, "subscription-test")
        .await
        .expect("post-EOF leader completed");

    let frames = output(&mut receiver).await;
    assert!(
        frames.matches(r#""type":"thinking_delta""#).count() >= 2,
        "post-EOF wait must remain visibly alive: {frames}"
    );
    assert!(frames.contains("done"));
    assert_valid_stream(&frames, Some("end_turn"));
}

#[tokio::test]
async fn stderr_can_finish_before_the_post_eof_leader() {
    let (sender, mut receiver) = channel();
    let options = short_post_eof_options();
    let mut process = child(concat!(
        "exec 2>&-; ",
        r#"printf '%s\n' '{"type":"result","subtype":"success","result":"done"}'; "#,
        "exec 1>&-; sleep 0.04",
    ));
    consume_subscription_stream_with_options(&mut process, &sender, &options, "subscription-test")
        .await
        .expect("stderr-first stream completed");

    let frames = output(&mut receiver).await;
    assert!(frames.contains("done"));
    assert_valid_stream(&frames, Some("end_turn"));
}

#[cfg(unix)]
#[tokio::test]
async fn stderr_drain_grace_kills_a_descriptor_holding_descendant() {
    let fixture = BackgroundSleepFixture::new();
    let script = fixture.stderr_holder_script(concat!(
        r#"printf '%s\n' '{"type":"result","subtype":"success","result":"done"}'; "#,
        "exit 0",
    ));
    let (sender, mut receiver) = channel();
    let options = short_post_eof_options();
    let mut process = child(&script);
    let (result, background) = tokio::join!(
        consume_subscription_stream_with_options(
            &mut process,
            &sender,
            &options,
            "subscription-test",
        ),
        fixture.release_after_pid(),
    );
    result.expect("valid result survives stderr drain cleanup");
    let mut background = background.expect("descriptor holder started");
    background.assert_exited().await;

    let frames = output(&mut receiver).await;
    assert!(frames.contains("done"));
    assert_valid_stream(&frames, Some("end_turn"));
}

#[cfg(unix)]
#[tokio::test]
async fn receiver_close_after_stdout_eof_kills_the_entire_process_group() {
    let fixture = BackgroundSleepFixture::new();
    let script = fixture.stderr_holder_script(concat!(
        r#"printf '%s\n' '{"type":"result","subtype":"success","result":"done"}'; "#,
        "exec 1>&-",
    ));
    let (sender, receiver) = channel();
    let options = short_post_eof_options();
    let mut process = child(&script);
    let wait_for_process = fixture.release_after_pid();
    tokio::pin!(wait_for_process);
    let consume = consume_subscription_stream_with_options(
        &mut process,
        &sender,
        &options,
        "subscription-test",
    );
    tokio::pin!(consume);
    let mut background = tokio::select! {
        background = &mut wait_for_process => background.expect("background process started"),
        result = &mut consume => panic!("stream ended before receiver close: {result:?}"),
    };
    drop(receiver);
    tokio::time::timeout(Duration::from_secs(2), consume)
        .await
        .expect("receiver-close cleanup is bounded")
        .expect("receiver-close cleanup succeeds");
    background.assert_exited().await;
}

#[cfg(unix)]
#[tokio::test]
async fn nonzero_exit_after_stderr_grace_preserves_the_diagnostic() {
    let fixture = BackgroundSleepFixture::new();
    let script = fixture.stderr_holder_script(concat!(
        "printf 'post-eof diagnostic' >&2; ",
        r#"printf '%s\n' '{"type":"result","subtype":"success","result":"done"}'; "#,
        "exit 7",
    ));
    let (sender, mut receiver) = channel();
    let options = short_post_eof_options();
    let mut process = child(&script);
    let (result, background) = tokio::join!(
        consume_subscription_stream_with_options(
            &mut process,
            &sender,
            &options,
            "subscription-test",
        ),
        fixture.release_after_pid(),
    );
    let error = result.expect_err("nonzero leader status must fail");
    assert!(error.to_string().contains("post-eof diagnostic"));
    let mut background = background.expect("descriptor holder started");
    background.assert_exited().await;
    assert_valid_stream(&output(&mut receiver).await, None);
}

#[cfg(unix)]
#[tokio::test]
async fn blocked_subagent_terminates_a_hanging_child_and_finishes_the_stream() {
    let (sender, mut receiver) = channel();
    let mut options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(2),
    );
    options.tools = vec!["Agent".to_owned()];
    options.tool_context = Some(SubscriptionToolContext::for_tests(
        Arc::new(AgentEffortIntents::default()),
        ModelCatalog::default(),
        None,
        "claude-haiku-4-5",
        Vec::new(),
        json!(null),
    ));
    let fixture = BackgroundSleepFixture::new();
    let blocked_event = r#"printf '%s\n' '{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"unsupported","name":"Agent","input":{"prompt":"work","subagent_type":"claudex-sonnet"}}]}}'"#;
    let mut child = child(&fixture.script(blocked_event));
    let attempt = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            consume_subscription_stream_with_options(
                &mut child,
                &sender,
                &options,
                "subscription-test",
            ),
            fixture.release_after_pid(),
        )
    })
    .await;
    if attempt.is_err() {
        crate::anthropic::subscription::terminate_subscription(&mut child)
            .await
            .expect("clean up timed-out blocked SubAgent fixture");
    }
    let (result, background) = attempt.expect("blocked SubAgent must terminate promptly");
    result.expect("blocked SubAgent must finish the parent stream");
    let mut background = background.expect("blocked fixture must record its background PID");
    assert!(child.try_wait().expect("query child status").is_some());
    background.assert_exited().await;
    let frames = output(&mut receiver).await;
    assert!(frames.contains("was not started. Continue without it."));
    assert!(frames.contains(r#""stop_reason":"end_turn""#));
    assert!(frames.contains("event: message_stop"));
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
async fn validates_process_exit_before_emitting_message_stop_and_preserves_failure_type() {
    let (sender, mut receiver) = channel();
    let error = consume_subscription_stream(
        child(concat!(
            r#"printf '%s\n' '{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"partial"}}}'; "#,
            r#"printf '%s\n' '{"type":"result","subtype":"success","result":"done"}'; "#,
            "printf 'Authentication failed' >&2; exit 7",
        )),
        &sender,
    )
    .await
    .expect_err("nonzero exit after result must fail");

    let failure = crate::anthropic::subscription::subscription_failure(&error)
        .expect("typed subscription failure must survive streamed output");
    assert_eq!(failure.status_hint(), 401);
    assert!(!failure.is_internal_retryable());
    assert!(!failure.is_outer_retryable());
    let frames = output(&mut receiver).await;
    assert!(frames.contains("partial"));
    assert_valid_stream(&frames, None);
}

#[cfg(unix)]
#[tokio::test]
async fn retries_an_empty_local_stream_exit_once() {
    let directory = tempfile::tempdir().expect("create stream retry fixture directory");
    let attempts = directory.path().join("attempts");
    let program = directory.path().join("stream-retry-fixture.sh");
    let attempts_path = attempts.display();
    fs::write(
        &program,
        format!(
            r#"#!/bin/sh
set -eu
if [ ! -f '{attempts_path}' ]; then
    printf 'attempt\n' > '{attempts_path}'
    cat >/dev/null
    exit 1
fi
printf 'attempt\n' >> '{attempts_path}'
cat >/dev/null
printf '%s\n' '{{"type":"result","subtype":"success","result":"STREAM_RETRIED_OK"}}'
"#
        ),
    )
    .expect("write stream retry fixture");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
        .expect("make stream retry fixture executable");
    let (sender, mut receiver) = channel();
    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(5),
    );

    run_subscription_stream(
        sender,
        program,
        "claude-haiku-4-5".to_owned(),
        "prompt".to_owned(),
        options,
    )
    .await;

    let frames = output(&mut receiver).await;
    assert!(frames.contains("STREAM_RETRIED_OK"));
    assert!(!frames.contains("event: error"));
    assert_eq!(
        fs::read_to_string(&attempts)
            .expect("attempt log")
            .lines()
            .count(),
        2
    );
}

#[cfg(unix)]
#[tokio::test]
async fn does_not_retry_a_structured_stream_502_internally() {
    let directory = tempfile::tempdir().expect("create stream failure fixture directory");
    let attempts = directory.path().join("attempts");
    let program = directory.path().join("stream-failure-fixture.sh");
    let attempts_path = attempts.display();
    fs::write(
        &program,
        format!(
            r#"#!/bin/sh
set -eu
cat >/dev/null
printf 'attempt\n' >> '{attempts_path}'
printf '%s\n' '{{"type":"result","subtype":"error","is_error":true,"result":"502 Bad Gateway"}}'
exit 1
"#
        ),
    )
    .expect("write stream failure fixture");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
        .expect("make stream failure fixture executable");
    let (sender, mut receiver) = channel();
    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(5),
    );

    run_subscription_stream(
        sender,
        program,
        "claude-haiku-4-5".to_owned(),
        "prompt".to_owned(),
        options,
    )
    .await;

    let frames = output(&mut receiver).await;
    assert!(frames.contains("502 Bad Gateway"));
    assert!(frames.contains("event: error"));
    assert_eq!(
        fs::read_to_string(&attempts)
            .expect("attempt log")
            .lines()
            .count(),
        1
    );
}

#[tokio::test]
async fn stops_cleanly_when_the_receiver_closes() {
    let (sender, receiver) = channel();
    drop(receiver);
    consume_subscription_stream(child("sleep 1"), &sender)
        .await
        .expect("closed response stream");
}

#[cfg(unix)]
#[tokio::test]
async fn ignores_prompt_write_failure_after_the_response_disconnects() {
    let directory = tempfile::tempdir().expect("create prompt fixture directory");
    let program = directory.path().join("exit-immediately.sh");
    fs::write(&program, "#!/bin/sh\nexit 0\n").expect("write prompt fixture");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
        .expect("make prompt fixture executable");
    let (sender, receiver) = channel();
    drop(receiver);
    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(5),
    );

    stream_subscription_model(
        &sender,
        &program,
        "model",
        &"x".repeat(1024 * 1024),
        &options,
    )
    .await
    .expect("closed response must cancel prompt cleanup without an API error");
}

#[cfg(unix)]
#[tokio::test]
async fn stream_timeout_terminates_the_entire_subscription_process_group() {
    let fixture = BackgroundSleepFixture::new();
    let program = fixture.program();
    let (sender, _receiver) = channel();
    let options = SubscriptionOptions::internal(
        Arc::new(tokio::sync::Semaphore::new(1)),
        Duration::from_secs(5),
    );

    let (result, background) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            stream_subscription_model(&sender, &program, "model", "prompt", &options),
            fixture.release_after_pid(),
        )
    })
    .await
    .expect("stalled subscription test must finish within its cleanup bound");
    let mut background = background.unwrap_or_else(|| {
        panic!("timeout fixture did not start its background process: {result:?}")
    });
    let error = result.expect_err("stalled subscription stream must time out");
    background.assert_exited().await;

    assert!(error.to_string().contains("timed out"));
    let failure = crate::anthropic::subscription::subscription_failure(&error)
        .expect("stream timeout must be typed");
    assert_eq!(failure.status_hint(), 424);
    assert!(!failure.is_internal_retryable());
    assert!(!failure.is_outer_retryable());
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

#[tokio::test]
async fn hydrates_auxiliary_claude_subagent_routes_without_adapter_fields_in_public_schema() {
    let mut model_catalog = ModelCatalog::default();
    model_catalog
        .set_auxiliary_worker_routes(vec![
            WorkerRoute::new(
                "claudex-haiku".to_owned(),
                "claude-haiku-4-5".to_owned(),
                "max".to_owned(),
            ),
            WorkerRoute::new(
                "claudex-sonnet".to_owned(),
                "claude-sonnet-5".to_owned(),
                "high".to_owned(),
            ),
            WorkerRoute::new(
                "custom-advisor".to_owned(),
                "claude-fable-5".to_owned(),
                "xhigh".to_owned(),
            ),
        ])
        .expect("auxiliary routes");
    let context = super::super::subscription::SubscriptionToolContext::for_tests(
        Arc::new(AgentEffortIntents::default()),
        model_catalog,
        None,
        "claude-opus-5",
        Vec::new(),
        json!(null),
    );
    let stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Agent".to_owned()],
        tool_context: Some(context),
        activity: SubscriptionActivity::default(),
    };

    for (id, agent) in [
        ("haiku", "claudex-haiku"),
        ("sonnet", "claudex-sonnet"),
        ("advisor", "custom-advisor"),
    ] {
        let public = stream
            .prepare_tool_input(
                "Agent",
                id,
                &json!({"prompt":"work", "subagent_type":agent}),
            )
            .expect("configured auxiliary SubAgent route");
        assert!(public.get("claudex_model").is_none());
        assert!(
            public["prompt"]
                .as_str()
                .is_some_and(|prompt| prompt.contains(id))
        );
    }
}

fn bare_subscription_stream(tools: Vec<String>) -> SubscriptionStream {
    SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools,
        tool_context: None,
        activity: SubscriptionActivity::default(),
    }
}

#[tokio::test]
async fn forward_tool_uses_stops_after_result_or_blocked_subagent() {
    let (sender, mut receiver) = channel();
    let envelope = json!({
        "type":"assistant",
        "parent_tool_use_id":null,
        "message":{"content":[{"type":"tool_use","id":"bash-1","name":"Bash","input":{"command":"true"}}]}
    });
    let mut finished = bare_subscription_stream(vec!["Bash".to_owned()]);
    finished.saw_result = true;
    assert!(
        !finished
            .forward_tool_uses(&sender, &envelope)
            .await
            .expect("finished stream ignores tools")
    );
    let mut blocked = bare_subscription_stream(vec!["Bash".to_owned()]);
    blocked.blocked_subagent = true;
    assert!(
        !blocked
            .forward_tool_uses(&sender, &envelope)
            .await
            .expect("blocked stream ignores tools")
    );
    assert!(output(&mut receiver).await.is_empty());
}

#[tokio::test]
async fn forward_tool_uses_closes_open_text_before_a_bash_tool() {
    let (sender, mut receiver) = channel();
    let mut stream = bare_subscription_stream(vec!["Bash".to_owned()]);
    stream.text_started = true;
    stream.text_closed = false;
    stream.next_index = 1;
    let forwarded = stream
        .forward_tool_uses(
            &sender,
            &json!({
                "type":"assistant",
                "parent_tool_use_id":null,
                "message":{"content":[{"type":"tool_use","id":"bash-open","name":"Bash","input":{"command":"true"}}]}
            }),
        )
        .await
        .expect("open text plus Bash");
    assert!(forwarded);
    assert!(stream.saw_tool_use);
    let output = output(&mut receiver).await;
    assert!(output.contains(r#""type":"tool_use""#));
    assert!(output.contains("bash-open"));
}

#[tokio::test]
async fn skips_foreign_task_stop_notice_after_a_live_agent_stop() {
    let (sender, mut receiver) = channel();
    let mut stream = bare_subscription_stream(vec!["TaskStop".to_owned()]);
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"stop-agent", "name":"TaskStop",
                    "input":{"task_id":"a4b2412c427ee5327"}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("forward live Agent TaskStop");
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"stop-foreign", "name":"TaskStop",
                    "input":{"task_id":"b13mjnjlj"}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("skip foreign TaskStop after a live stop");
    assert!(stream.saw_tool_use);
    let output = output(&mut receiver).await;
    assert!(output.contains("a4b2412c427ee5327"));
    assert!(!output.contains("TaskStop skipped"));
}

#[tokio::test]
async fn skips_stale_task_output_notice_after_a_live_output() {
    let (sender, mut receiver) = channel();
    let mut stream = bare_subscription_stream(vec!["TaskOutput".to_owned()]);
    stream.tool_context = Some(SubscriptionToolContext::for_tests(
        Arc::new(AgentEffortIntents::default()),
        ModelCatalog::default(),
        None,
        "parent-model",
        vec![json!({
            "role":"user",
            "content":[{
                "type":"tool_result",
                "tool_use_id":"toolu_live",
                "content":[{"type":"text","text":"Async agent launched successfully.\nagentId: a4496564387a2561f"}]
            }]
        })],
        json!(null),
    ));
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"live-output", "name":"TaskOutput",
                    "input":{"task_id":"a4496564387a2561f","block":false}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("forward live TaskOutput");
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"stale-output", "name":"TaskOutput",
                    "input":{"task_id":"a3d7f2ca50556c9e5","block":false}
                }]}
            })
            .to_string(),
        )
        .await
        .expect("skip stale TaskOutput after a live output");
    assert!(stream.saw_tool_use);
    let output = output(&mut receiver).await;
    assert!(output.contains("a4496564387a2561f"));
    assert!(!output.contains("TaskOutput skipped"));
}

#[tokio::test]
async fn resumes_an_existing_subscription_agent_without_duplicate_scope_check() {
    let (sender, mut receiver) = channel();
    let mut context = explicit_subscription_tool_context();
    context.session_id = Some("session-resume".to_owned());
    let mut stream = bare_subscription_stream(vec!["Agent".to_owned(), "Task".to_owned()]);
    stream.tool_context = Some(context);
    stream
        .handle_line(
            &sender,
            &json!({
                "type":"assistant", "parent_tool_use_id":null,
                "message":{"content":[{
                    "type":"tool_use", "id":"resume-agent", "name":"Agent",
                    "input":{
                        "prompt":"scope a",
                        "subagent_type":"claudex-gpt-spark",
                        "resume":"worker-a"
                    }
                }]}
            })
            .to_string(),
        )
        .await
        .expect("resume Agent launch");
    assert!(stream.saw_tool_use);
    let output = output(&mut receiver).await;
    assert!(output.contains(r#""type":"tool_use""#) || output.contains("Task"));
}

#[tokio::test]
async fn suppresses_text_deltas_after_tool_use_or_blocked_subagent() {
    let (sender, mut receiver) = channel();
    let delta = r#"{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"late"}}}"#;

    let mut after_tool = bare_subscription_stream(Vec::new());
    after_tool.saw_tool_use = true;
    after_tool
        .handle_line(&sender, delta)
        .await
        .expect("tool-use stream ignores late text");
    assert!(!after_tool.text_started);

    let mut blocked = bare_subscription_stream(Vec::new());
    blocked.blocked_subagent = true;
    blocked
        .handle_line(&sender, delta)
        .await
        .expect("blocked stream ignores late text");
    assert!(!blocked.text_started);
    assert!(output(&mut receiver).await.is_empty());
}

#[test]
fn prepare_tool_input_rejects_disabled_subagent_models() {
    let mut context = SubscriptionToolContext::for_tests(
        Arc::new(AgentEffortIntents::default()),
        ModelCatalog::default(),
        None,
        "parent-model",
        vec![json!({"role":"user","content":"launch worker"})],
        json!(null),
    );
    context
        .disabled_subagent_models
        .insert("gpt-test".to_owned());
    let stream = SubscriptionStream {
        text_started: false,
        text_closed: false,
        saw_tool_use: false,
        launch_fanout_open: false,
        seen_tool_ids: HashSet::new(),
        blocked_subagent: false,
        saw_result: false,
        next_index: 0,
        tools: vec!["Agent".to_owned()],
        tool_context: Some(context),
        activity: SubscriptionActivity::default(),
    };
    let error = stream
        .prepare_tool_input(
            "Agent",
            "agent-disabled",
            &json!({"prompt":"do work","claudex_model":"gpt-test"}),
        )
        .expect_err("disabled model must be rejected");
    assert!(
        error
            .to_string()
            .contains(super::super::agent_route_validation::BLOCKED_SUBAGENT_NOTICE)
            || error.to_string().to_lowercase().contains("disabled")
            || error.to_string().contains("BLOCKED")
            || error.to_string().contains("blocked"),
        "{error:#}"
    );
}
