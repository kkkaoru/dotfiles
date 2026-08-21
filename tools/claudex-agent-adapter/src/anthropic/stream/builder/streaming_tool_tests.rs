use std::convert::Infallible;

use axum::body::Bytes;
use serde_json::json;
use tokio::sync::mpsc;

use super::super::SegmentBuilder;
use crate::anthropic::stream::subagent_live_view::SubAgentLiveView;

#[tokio::test]
async fn empty_read_start_does_not_emit_tool_use_until_path_is_ready() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1)
        .with_subagent(true)
        .with_primed_thinking();
    builder
        .start_executable_tool_use_card("call-1", "Read", Some(&sender))
        .await
        .expect("hold Read card");

    let mut live = SubAgentLiveView::default();
    live.ingest_available(&mut receiver);
    assert!(live.turn_still_open());
    assert!(live.visible_tool_use.is_empty());
    assert!(live.visible_thinking.is_empty());
    assert!(!live.visible_thinking.contains('▶'));
    assert!(!builder.has_external_tool_calls());
    assert!(builder.streaming_tool.is_some());

    builder
        .append_native_tool_use_delta("call-1", "{\"path\":\"CLAUDE.md\"}", Some(&sender))
        .await
        .expect("Read argument delta");
    live.ingest_available(&mut receiver);
    assert!(live.turn_still_open());
    assert_eq!(live.visible_tool_use, vec!["Read".to_owned()]);
    assert!(builder.has_external_tool_calls());
    drop(sender);
}

#[tokio::test]
async fn agent_tool_start_does_not_open_a_live_card() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("agent-1", "Agent", Some(&sender))
        .await
        .expect("skip Agent card");
    assert!(builder.streaming_tool.is_none());
    assert!(!builder.has_external_tool_calls());
    drop(sender);
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn second_start_is_ignored_while_a_card_is_open() {
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Read", None)
        .await
        .expect("first card");
    builder
        .start_executable_tool_use_card("call-2", "Bash", None)
        .await
        .expect("second start ignored");
    assert_eq!(
        builder
            .streaming_tool
            .as_ref()
            .map(|open| open.call_id.as_str()),
        Some("call-1")
    );
    assert!(builder.blocks.is_empty());
}

#[tokio::test]
#[expect(
    clippy::excessive_nesting,
    reason = "SSE frame audit intentionally mirrors nested protocol structure"
)]
async fn tool_after_closed_thinking_does_not_skip_or_stop_an_unstarted_index() {
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .thinking
        .progress_status_keep_open(&mut builder.blocks, "plan the edit\n", Some(&sender))
        .await
        .expect("stream thinking");
    builder
        .thinking
        .close(&mut builder.blocks, Some(&sender))
        .await
        .expect("close thinking on reasoning/complete");
    assert!(
        !builder.thinking.is_open(),
        "thinking_end must close the live thought before the tool card"
    );
    builder.pending_reasoning =
        "plan the edit\nand extra CoT that is not a substring of the live block".to_owned();
    builder
        .start_executable_tool_use_card("call-1", "Read", Some(&sender))
        .await
        .expect("start Read after closed thinking");
    builder
        .append_native_tool_use_delta("call-1", "{\"file_path\":\"CLAUDE.md\"}", Some(&sender))
        .await
        .expect("Read path ready");
    drop(sender);

    let mut next_index = 0_u64;
    let mut open: Option<u64> = None;
    let mut started = Vec::new();
    let mut frames = Vec::new();
    let mut drain = receiver;
    while let Some(frame) = drain.recv().await {
        let raw = String::from_utf8(frame.expect("frame").to_vec()).expect("utf8");
        frames.push(raw.clone());
        for chunk in raw.split("\n\n") {
            let Some(data) = chunk.lines().find_map(|line| line.strip_prefix("data: ")) else {
                continue;
            };
            let Ok(payload) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            match payload.get("type").and_then(serde_json::Value::as_str) {
                Some("content_block_start") => {
                    let index = payload["index"].as_u64().expect("start index");
                    assert_eq!(
                        index, next_index,
                        "SSE indexes must stay contiguous: {frames:?}"
                    );
                    assert!(open.replace(index).is_none(), "nested start: {frames:?}");
                    next_index += 1;
                    started.push(payload["content_block"]["type"].clone());
                }
                Some("content_block_delta") => {
                    let index = payload["index"].as_u64().expect("delta index");
                    assert_eq!(open, Some(index), "delta for missing block: {frames:?}");
                }
                Some("content_block_stop") => {
                    let index = payload["index"].as_u64().expect("stop index");
                    assert_eq!(
                        open.take(),
                        Some(index),
                        "content_block_stop without start: {frames:?}"
                    );
                }
                _ => {}
            }
        }
    }
    assert_eq!(
        started,
        vec![json!("thinking"), json!("tool_use")],
        "Read must follow the closed thought at the next index: {frames:?}"
    );
}

#[tokio::test]
async fn unmatched_delta_is_ignored() {
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .append_native_tool_use_delta("missing", "{\"a\":1}", None)
        .await
        .expect("no open tool");
    builder
        .start_executable_tool_use_card("call-1", "Grep", None)
        .await
        .expect("open Grep");
    builder
        .append_native_tool_use_delta("other", "x", None)
        .await
        .expect("wrong id ignored");
    builder
        .append_native_tool_use_delta("call-1", "", None)
        .await
        .expect("empty delta ignored");
    assert_eq!(
        builder
            .streaming_tool
            .as_ref()
            .map(|open| open.partial_json.as_str()),
        Some("")
    );
}

#[tokio::test]
async fn incomplete_bash_argument_deltas_are_not_sent_to_claude_code() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Bash", Some(&sender))
        .await
        .expect("start Bash");
    builder
        .append_native_tool_use_delta("call-1", "{\"command\":\"cat", Some(&sender))
        .await
        .expect("incomplete delta");
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        !output.contains("tool_use"),
        "truncated Bash must not start a tool_use card: {output}"
    );
    assert!(
        !output.contains("input_json_delta"),
        "truncated Bash JSON must not be flushed: {output}"
    );
}

#[tokio::test]
async fn complete_bash_argument_json_is_flushed_before_tool_end() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Bash", Some(&sender))
        .await
        .expect("start Bash");
    builder
        .append_native_tool_use_delta("call-1", "{\"command\":\"ls\"}", Some(&sender))
        .await
        .expect("complete delta");
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(output.contains("tool_use"));
    assert!(output.contains("input_json_delta"));
    assert!(
        output.contains("{\\\"command\\\":\\\"ls\\\"}") || output.contains("\"command\":\"ls\"")
    );
}

#[tokio::test]
async fn finish_stops_an_open_streaming_tool_use() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Write", Some(&sender))
        .await
        .expect("open Write");
    builder.finish(Some(&sender)).await.expect("finish");
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        output.contains("content_block_stop"),
        "finish must close the live tool_use block: {output}"
    );
    assert!(builder.streaming_tool.is_none());
}

#[tokio::test]
async fn malformed_delta_event_errors() {
    let mut builder = SegmentBuilder::new(1);
    let error = builder
        .delta_native_tool_use(&json!({"params":{}}), None)
        .await
        .expect_err("missing callId");
    assert!(error.to_string().contains("callId missing"));
}

#[tokio::test]
async fn empty_bash_never_starts_a_tool_use_card() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Bash", Some(&sender))
        .await
        .expect("hold Bash");
    builder
        .append_native_tool_use_delta("call-1", "{}", Some(&sender))
        .await
        .expect("empty object held");
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        !output.contains("tool_use"),
        "empty Bash must not start a tool_use card: {output}"
    );
    assert!(
        !output.contains("input_json_delta"),
        "empty Bash JSON must not be flushed: {output}"
    );
    assert!(!builder.has_external_tool_calls());
}

#[tokio::test]
async fn empty_send_message_object_is_not_flushed_as_complete_input() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "SendMessage", Some(&sender))
        .await
        .expect("start SendMessage");
    builder
        .append_native_tool_use_delta("call-1", "{}", Some(&sender))
        .await
        .expect("empty object held");
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        !output.contains("tool_use"),
        "empty SendMessage must not start a tool_use card: {output}"
    );
    assert!(
        !output.contains("input_json_delta"),
        "empty SendMessage JSON must not be flushed: {output}"
    );
}

#[tokio::test]
async fn finish_rejects_incomplete_bash_without_flushing_empty_object() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Bash", Some(&sender))
        .await
        .expect("start Bash");
    builder
        .append_native_tool_use_delta("call-1", "{}", Some(&sender))
        .await
        .expect("empty object held");
    let error = match builder.finish(Some(&sender)).await {
        Err(error) => error,
        Ok(_) => panic!("incomplete Bash must fail"),
    };
    assert_eq!(
        error.to_string(),
        "Incomplete Bash tool JSON was not flushed; a non-empty command is required."
    );
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        !output.contains("tool_use"),
        "incomplete Bash must not start a tool_use card: {output}"
    );
    assert!(
        !output.contains("input_json_delta"),
        "empty Bash JSON must not be flushed on finish: {output}"
    );
}

#[tokio::test]
async fn empty_tool_json_circuit_trips_after_three_empty_bash_payloads() {
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Bash", None)
        .await
        .expect("start 1");
    builder
        .append_native_tool_use_delta("call-1", "{}", None)
        .await
        .expect("empty 1 held");
    assert!(builder.take_streaming_tool("call-1").is_some());

    builder
        .start_executable_tool_use_card("call-2", "Bash", None)
        .await
        .expect("start 2");
    builder
        .append_native_tool_use_delta("call-2", "{}", None)
        .await
        .expect("empty 2 held");
    assert!(builder.take_streaming_tool("call-2").is_some());

    builder
        .start_executable_tool_use_card("call-3", "Bash", None)
        .await
        .expect("start 3");
    let error = builder
        .append_native_tool_use_delta("call-3", "{}", None)
        .await
        .expect_err("circuit");
    assert_eq!(
        error.to_string(),
        "Stopped emitting tool_use after 3 consecutive empty or invalid JSON payloads."
    );
}

#[tokio::test]
async fn empty_read_never_starts_a_tool_use_card() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Read", Some(&sender))
        .await
        .expect("hold Read");
    builder
        .append_native_tool_use_delta("call-1", "{}", Some(&sender))
        .await
        .expect("empty object held");
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        !output.contains("tool_use"),
        "empty Read must not start a tool_use card: {output}"
    );
    assert!(
        !output.contains("input_json_delta"),
        "empty Read JSON must not be flushed: {output}"
    );
    assert!(!builder.has_external_tool_calls());
}

#[tokio::test]
async fn empty_grep_never_starts_a_tool_use_card() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Grep", Some(&sender))
        .await
        .expect("hold Grep");
    builder
        .append_native_tool_use_delta("call-1", "{}", Some(&sender))
        .await
        .expect("empty object held");
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        !output.contains("tool_use"),
        "empty Grep must not start a tool_use card: {output}"
    );
    assert!(
        !output.contains("input_json_delta"),
        "empty Grep JSON must not be flushed: {output}"
    );
    assert!(!builder.has_external_tool_calls());
}

#[tokio::test]
async fn empty_glob_never_starts_a_tool_use_card() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Glob", Some(&sender))
        .await
        .expect("hold Glob");
    builder
        .append_native_tool_use_delta("call-1", "{}", Some(&sender))
        .await
        .expect("empty object held");
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        !output.contains("tool_use"),
        "empty Glob must not start a tool_use card: {output}"
    );
    assert!(
        !output.contains("input_json_delta"),
        "empty Glob JSON must not be flushed: {output}"
    );
    assert!(!builder.has_external_tool_calls());
}

#[tokio::test]
async fn complete_read_argument_json_is_flushed_before_tool_end() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Read", Some(&sender))
        .await
        .expect("start Read");
    builder
        .append_native_tool_use_delta("call-1", "{\"file_path\":\"CLAUDE.md\"}", Some(&sender))
        .await
        .expect("complete delta");
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(output.contains("tool_use"));
    assert!(output.contains("input_json_delta"));
    assert!(
        output.contains("{\\\"file_path\\\":\\\"CLAUDE.md\\\"}")
            || output.contains("\"file_path\":\"CLAUDE.md\"")
    );
}

#[tokio::test]
async fn complete_grep_argument_json_is_flushed_before_tool_end() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Grep", Some(&sender))
        .await
        .expect("start Grep");
    builder
        .append_native_tool_use_delta("call-1", "{\"pattern\":\"tool_use\"}", Some(&sender))
        .await
        .expect("complete delta");
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(output.contains("tool_use"));
    assert!(output.contains("input_json_delta"));
    assert!(
        output.contains("{\\\"pattern\\\":\\\"tool_use\\\"}")
            || output.contains("\"pattern\":\"tool_use\"")
    );
}

#[tokio::test]
async fn finish_rejects_incomplete_read_without_flushing_empty_object() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Read", Some(&sender))
        .await
        .expect("start Read");
    builder
        .append_native_tool_use_delta("call-1", "{}", Some(&sender))
        .await
        .expect("empty object held");
    let error = match builder.finish(Some(&sender)).await {
        Err(error) => error,
        Ok(_) => panic!("incomplete Read must fail"),
    };
    assert_eq!(
        error.to_string(),
        "Incomplete Read tool JSON was not flushed; a non-empty file_path or path is required."
    );
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        !output.contains("tool_use"),
        "incomplete Read must not start a tool_use card: {output}"
    );
    assert!(
        !output.contains("input_json_delta"),
        "empty Read JSON must not be flushed on finish: {output}"
    );
}

#[tokio::test]
async fn finish_rejects_incomplete_grep_without_flushing_empty_object() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Grep", Some(&sender))
        .await
        .expect("start Grep");
    builder
        .append_native_tool_use_delta("call-1", "{}", Some(&sender))
        .await
        .expect("empty object held");
    let error = match builder.finish(Some(&sender)).await {
        Err(error) => error,
        Ok(_) => panic!("incomplete Grep must fail"),
    };
    assert_eq!(
        error.to_string(),
        "Incomplete Grep tool JSON was not flushed; a non-empty pattern is required."
    );
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        !output.contains("tool_use"),
        "incomplete Grep must not start a tool_use card: {output}"
    );
    assert!(
        !output.contains("input_json_delta"),
        "empty Grep JSON must not be flushed on finish: {output}"
    );
}

#[tokio::test]
async fn finish_rejects_incomplete_glob_without_flushing_empty_object() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .start_executable_tool_use_card("call-1", "Glob", Some(&sender))
        .await
        .expect("start Glob");
    builder
        .append_native_tool_use_delta("call-1", "{}", Some(&sender))
        .await
        .expect("empty object held");
    let error = match builder.finish(Some(&sender)).await {
        Err(error) => error,
        Ok(_) => panic!("incomplete Glob must fail"),
    };
    assert_eq!(
        error.to_string(),
        "Incomplete Glob tool JSON was not flushed; a non-empty pattern is required."
    );
    drop(sender);
    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
    }
    assert!(
        !output.contains("tool_use"),
        "incomplete Glob must not start a tool_use card: {output}"
    );
    assert!(
        !output.contains("input_json_delta"),
        "empty Glob JSON must not be flushed on finish: {output}"
    );
}
