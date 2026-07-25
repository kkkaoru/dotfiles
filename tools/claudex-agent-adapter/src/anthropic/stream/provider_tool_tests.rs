#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use axum::body::Bytes;
    use serde_json::json;
    use std::convert::Infallible;
    use tokio::sync::mpsc;

    use super::*;

    #[tokio::test]
    async fn builds_provider_progress_without_executable_tool_use_blocks() {
        const SAMPLE_INPUT_TOKEN_COUNT: u64 = 1;
        const EXPECTED_PROVIDER_CALLS: usize = 2;
        const EXPECTED_PROGRESS_BLOCKS: usize = 1;
        let mut builder = SegmentBuilder::new(SAMPLE_INPUT_TOKEN_COUNT);
        assert!(builder.provider_tool_call(&json!({}), None).await.is_err());
        assert!(
            builder
                .provider_tool_call(&json!({"params":{}}), None)
                .await
                .is_err()
        );
        builder
            .provider_tool_call(
                &json!({"params":{"callId":"provider-read","tool":"Read","arguments":{"path":"a"}}}),
                None,
            )
            .await
            .expect("provider progress");
        // ACP may describe the same call again in an incremental update.
        builder
            .provider_tool_call(
                &json!({"params":{"callId":"provider-read","tool":"Read","title":"Read a"}}),
                None,
            )
            .await
            .expect("duplicate provider progress");
        builder
            .provider_tool_call(
                &json!({"params":{"callId":"provider-search","title":"Search docs"}}),
                None,
            )
            .await
            .expect("default provider progress");

        // Progress lives in ephemeral thinking, not open answer text.
        assert_eq!(builder.blocks.len(), EXPECTED_PROGRESS_BLOCKS);
        assert!(builder.open_text_block.is_none());
        let thinking = builder.blocks[0]["thinking"].as_str().expect("status thinking");
        assert!(thinking.contains("▶ Read"));
        assert!(thinking.contains("▶ Search docs"));
        assert!(!thinking.contains("tool_use"));
        assert_eq!(
            builder.provider_tool_calls.len(),
            EXPECTED_PROVIDER_CALLS
        );
        let segment = builder.finish(None).await.expect("segment");
        assert!(
            segment.blocks.is_empty(),
            "provider progress must not remain in committed answer blocks"
        );
    }

    #[tokio::test]
    async fn streams_provider_progress_and_all_status_variants() {
        const SAMPLE_INPUT_TOKEN_COUNT: u64 = 1;
        const STREAM_CHANNEL_CAPACITY: usize = 32;
        const EXPECTED_PROGRESS_FRAMES: usize = 6;
        let (sender, mut receiver) =
            mpsc::channel::<Result<Bytes, Infallible>>(STREAM_CHANNEL_CAPACITY);
        let mut builder = SegmentBuilder::new(SAMPLE_INPUT_TOKEN_COUNT);
        builder
            .provider_tool_call(
                &json!({"params":{"callId":"1","tool":"Bash","arguments":{}}}),
                Some(&sender),
            )
            .await
            .expect("stream progress");
        builder
            .provider_tool_update(
                &json!({"params":{"status":"failed","title":"Build","output":{"code":1}}}),
                Some(&sender),
            )
            .await
            .expect("failed status");
        builder
            .provider_tool_update(
                &json!({"params":{"status":"completed","title":"Read","output":" done "}}),
                Some(&sender),
            )
            .await
            .expect("completed status");
        builder
            .provider_tool_update(&json!({"params":{"status":"completed"}}), Some(&sender))
            .await
            .expect("empty completed status");
        builder
            .provider_tool_update(&json!({"params":{"status":"pending"}}), Some(&sender))
            .await
            .expect("ignored status");
        assert!(
            builder
                .provider_tool_update(&json!({}), None)
                .await
                .is_err()
        );
        let segment = builder.finish(Some(&sender)).await.expect("segment");
        drop(sender);

        assert!(segment.blocks.iter().all(|block| block["type"] != "tool_use"));
        // Live frames carried progress; committed segment stays answer-clean.
        assert!(
            segment.blocks.is_empty(),
            "provider progress is ephemeral and stripped on finish"
        );
        let mut output = String::new();
        let mut frame_count = 0;
        while let Some(frame) = receiver.recv().await {
            frame_count += 1;
            output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
        }
        assert!(output.contains("▶ Bash"));
        assert!(output.contains("✗ Build"));
        assert!(output.contains("✓ Read"));
        assert_eq!(frame_count, EXPECTED_PROGRESS_FRAMES);
    }

    #[tokio::test]
    async fn renders_update_only_tools_once_and_reuses_their_titles() {
        let mut builder = SegmentBuilder::new(1);
        for status in ["pending", "in_progress"] {
            builder
                .provider_tool_update(
                    &json!({"params":{
                        "callId":"update-only",
                        "status":status,
                        "title":"WebFetch"
                    }}),
                    None,
                )
                .await
                .expect("provider update");
        }
        builder
            .provider_tool_update(
                &json!({"params":{
                    "callId":"update-only",
                    "status":"completed",
                    "output":"done"
                }}),
                None,
            )
            .await
            .expect("provider completion");
        // Before finish, progress is in thinking; after finish it is stripped.
        assert_eq!(
            builder.blocks[0]["thinking"]
                .as_str()
                .expect("progress thinking")
                .matches("▶ WebFetch")
                .count(),
            1
        );
        let segment = builder.finish(None).await.expect("segment");
        assert!(segment.blocks.is_empty());
    }

    #[test]
    fn previews_and_truncates_status_output() {
        const UNREACHED_PREVIEW_CHAR_LIMIT: usize = 20;
        const TRUNCATED_PREVIEW_CHAR_LIMIT: usize = 3;
        assert_eq!(output_preview(Some(&json!("text")), "fallback"), "text");
        assert_eq!(
            output_preview(Some(&json!({"a":1})), "fallback"),
            "{\"a\":1}"
        );
        assert_eq!(output_preview(None, "fallback"), "fallback");
        assert_eq!(
            truncate_for_status("  short  ", UNREACHED_PREVIEW_CHAR_LIMIT),
            "short"
        );
        assert_eq!(
            truncate_for_status("abcdef", TRUNCATED_PREVIEW_CHAR_LIMIT),
            "abc…"
        );
    }
}
