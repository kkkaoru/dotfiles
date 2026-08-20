use serde_json::json;

use super::*;

#[tokio::test]
async fn tool_handoff_closes_keepalive_thinking_before_executable_tool_use() {
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .thinking
        .progress_status_keep_open(&mut builder.blocks, "▶ Read\n", None)
        .await
        .expect("open keepalive thinking");
    assert!(
        builder.thinking.is_open(),
        "precondition: thinking stays open during keepalive"
    );

    builder
        .close_blocks_for_finish(true, None)
        .await
        .expect("close thinking before tool_use");

    assert!(
        !builder.thinking.is_open(),
        "Codex Read/Bash tool_use must not ride an open thinking block (Slithering)"
    );
    assert_eq!(builder.blocks[0]["type"], "thinking");
    assert_eq!(builder.blocks.len(), 1);
}

#[tokio::test]
async fn subagent_finish_with_tool_use_closes_thinking_and_reports_tool_use() {
    let mut builder = SegmentBuilder::new(1).with_subagent(true);
    builder
        .thinking
        .progress_status_keep_open(&mut builder.blocks, "▶ Read\n", None)
        .await
        .expect("open keepalive thinking");
    builder.external_tool_calls = 1;
    builder.blocks.push(json!({
        "type": "tool_use",
        "id": "toolu_read",
        "name": "Read",
        "input": {"file_path": ".gitignore"}
    }));

    let segment = builder.finish(None).await.expect("finish tool handoff");

    assert!(
        !builder.thinking.is_open(),
        "thinking must be closed before the committed Read tool_use"
    );
    assert_eq!(segment.stop_reason, "tool_use");
    assert_eq!(segment.blocks.last().unwrap()["type"], "tool_use");
    assert_eq!(segment.blocks.last().unwrap()["name"], "Read");
}

#[tokio::test]
async fn commit_pending_reasoning_skips_a_transcript_already_holding_the_same_text() {
    let mut builder = SegmentBuilder::new(1);
    builder.blocks.push(json!({
        "type": "thinking",
        "thinking": "already covered reasoning text"
    }));
    builder.pending_reasoning = "already covered reasoning text".to_owned();

    builder
        .commit_pending_reasoning_for_transcript(None)
        .await
        .expect("a skipped commit is not an error");

    assert!(
        builder.pending_reasoning.is_empty(),
        "the pending buffer still drains even when the commit itself is skipped"
    );
    assert_eq!(
        builder.blocks.len(),
        1,
        "no duplicate thinking block is appended for text already on the transcript"
    );
}

#[tokio::test]
async fn commit_pending_reasoning_does_not_invent_an_unstarted_sse_block() {
    let mut builder = SegmentBuilder::new(1);
    builder
        .blocks
        .push(json!({"type":"text","text":"unrelated"}));
    builder.pending_reasoning = "fresh reasoning text".to_owned();
    builder
        .commit_pending_reasoning_for_transcript(None)
        .await
        .expect("commit without a live stream");
    assert!(builder.pending_reasoning.is_empty());
    assert!(
        builder
            .blocks
            .iter()
            .all(|block| block.get("type").and_then(serde_json::Value::as_str) != Some("thinking")),
        "stream=None must not append a thinking block Claude Code never started: {:?}",
        builder.blocks
    );
}

#[tokio::test]
async fn commit_pending_reasoning_writes_when_transcript_does_not_already_hold_it() {
    use std::convert::Infallible;

    use axum::body::Bytes;
    use tokio::sync::mpsc;

    let (sender, _receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
    let mut builder = SegmentBuilder::new(1);
    builder
        .blocks
        .push(json!({"type":"text","text":"unrelated"}));
    builder.pending_reasoning = "fresh reasoning text".to_owned();
    builder
        .commit_pending_reasoning_for_transcript(Some(&sender))
        .await
        .expect("commit fresh reasoning");
    assert!(builder.pending_reasoning.is_empty());
    assert!(
        builder
            .blocks
            .iter()
            .any(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("thinking"))
    );
}

#[test]
fn thinking_contains_pending_rejects_blank_needles_and_non_thinking_blocks() {
    assert!(!thinking_contains_pending(&[], "   "));
    assert!(!thinking_contains_pending(
        &[json!({"type":"text","thinking":"needle"})],
        "needle"
    ));
}

#[tokio::test]
async fn blocked_subagent_notice_finishes_end_turn_despite_provider_tool_use() {
    let notice = "SubAgent model `qwen3.8-max-preview` is disabled by policy and was not launched.";
    let mut builder = SegmentBuilder::new(1);
    builder
        .emit_blocked_notice(notice, None)
        .await
        .expect("blocked notice");
    builder
        .update_provider_stop_reason(&json!({
            "params":{"turn":{"providerStopReason":"tool_use"}}
        }))
        .expect("provider stopped for the blocked Agent tool");

    let segment = builder
        .finish(None)
        .await
        .expect("blocked SubAgent must complete without a protocol error");

    assert_eq!(segment.stop_reason, "end_turn");
    assert_eq!(segment.blocks.len(), 1);
    assert_eq!(segment.blocks[0]["type"], "text");
    assert_eq!(segment.blocks[0]["text"], notice);
    assert!(
        segment.blocks[0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("was not launched")
                || text.contains("Continue without it.")
                || text.contains("disabled by policy")),
        "notice must stay visible: {}",
        segment.blocks[0]["text"]
    );
}

#[tokio::test]
async fn blocked_subagent_notice_streams_message_stop_after_finish() {
    use std::convert::Infallible;

    use axum::body::Bytes;
    use tokio::sync::mpsc;

    use crate::anthropic::stream::protocol::send_stream_completion;

    let notice = "The requested SubAgent model is disabled by policy, so it was not started. Continue without it.";
    let mut builder = SegmentBuilder::new(1);
    let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    builder
        .emit_blocked_notice(notice, Some(&sender))
        .await
        .expect("stream notice");
    builder
        .update_provider_stop_reason(&json!({
            "params":{"turn":{"providerStopReason":"tool_use"}}
        }))
        .expect("tool_use stop");
    let segment = builder.finish(Some(&sender)).await.expect("finish ok");
    send_stream_completion(&sender, &segment).await;
    drop(sender);

    let mut output = String::new();
    while let Some(frame) = receiver.recv().await {
        output.push_str(&String::from_utf8_lossy(&frame.expect("infallible")));
    }
    assert!(output.contains("Continue without it."));
    assert!(output.contains(r#""stop_reason":"end_turn""#));
    assert!(output.contains("event: message_stop"));
    assert!(!output.contains("Server error"));
    assert!(!output.contains("without emitting a tool call"));
}
