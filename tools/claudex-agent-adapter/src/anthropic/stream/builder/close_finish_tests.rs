use serde_json::json;

use super::*;

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
