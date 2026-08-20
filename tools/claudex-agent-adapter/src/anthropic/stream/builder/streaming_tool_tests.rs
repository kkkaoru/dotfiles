use std::convert::Infallible;

use axum::body::Bytes;
use serde_json::json;
use tokio::sync::mpsc;

use super::super::SegmentBuilder;
use crate::anthropic::stream::subagent_live_view::SubAgentLiveView;

#[tokio::test]
async fn tool_start_paints_native_tool_use_before_arguments_complete() {
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let mut builder = SegmentBuilder::new(1)
        .with_subagent(true)
        .with_primed_thinking();
    builder
        .start_executable_tool_use_card("call-1", "Read", Some(&sender))
        .await
        .expect("start Read card");

    let mut live = SubAgentLiveView::default();
    live.ingest_available(&mut receiver);
    assert!(live.turn_still_open());
    assert_eq!(live.visible_tool_use, vec!["Read".to_owned()]);
    assert!(live.visible_thinking.is_empty());
    assert!(!live.visible_thinking.contains('▶'));
    assert!(builder.has_external_tool_calls());
    assert!(builder.streaming_tool.is_some());

    builder
        .append_native_tool_use_delta("call-1", "{\"path\":\"CLAUDE.md\"}", Some(&sender))
        .await
        .expect("Read argument delta");
    live.ingest_available(&mut receiver);
    assert!(live.turn_still_open());
    assert_eq!(live.visible_tool_use, vec!["Read".to_owned()]);
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
    assert_eq!(builder.blocks.len(), 1);
    assert_eq!(builder.blocks[0]["name"], "Read");
}

#[tokio::test]
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
