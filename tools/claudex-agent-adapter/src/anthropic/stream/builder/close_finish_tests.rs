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
