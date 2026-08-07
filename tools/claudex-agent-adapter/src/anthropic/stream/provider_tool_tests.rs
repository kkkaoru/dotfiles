#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
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

        // Progress is visible before commit (never executable tool_use).
        assert_eq!(builder.blocks.len(), EXPECTED_PROGRESS_BLOCKS);
        assert!(builder.open_text_block.is_some());
        let text = builder.open_text_block.as_ref().expect("progress text").1.as_str();
        assert!(text.contains("▶ Read"));
        assert!(text.contains("a"));
        assert!(text.contains("▶ Search docs"));
        assert!(!text.contains("tool_use"));
        assert_eq!(
            builder.provider_tool_calls.len(),
            EXPECTED_PROVIDER_CALLS
        );
        let segment = builder.finish(None).await.expect("segment");
        assert_eq!(segment.blocks.len(), 1);
        let committed = segment.blocks[0]["text"].as_str().expect("committed text");
        assert!(committed.trim().is_empty());
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
        // Live frames carry progress; committed output is transcript-clean.
        let committed = segment.blocks[0]["text"].as_str().expect("committed progress");
        assert!(committed.trim().is_empty());
        let (frame_count, output) = collect_frames(&mut receiver).await;
        assert!(output.contains("▶ Bash"));
        assert!(output.contains("✗ Build"));
        assert!(output.contains("✓ Read"));
        assert!(!output.contains(" done "));
        assert_eq!(frame_count, EXPECTED_PROGRESS_FRAMES);
    }

    #[tokio::test]
    async fn completed_progress_omits_large_tool_bodies() {
        let mut builder = SegmentBuilder::new(1);
        builder
            .provider_tool_call(
                &json!({"params":{
                    "callId":"shell-1",
                    "tool":"run_terminal_command",
                    "title":"run_terminal_command",
                    "arguments":{"command":"pwd && git status && git branch --show-current && ls -la"}
                }}),
                None,
            )
            .await
            .expect("start");
        builder
            .provider_tool_update(
                &json!({"params":{
                    "callId":"shell-1",
                    "status":"completed",
                    "title":"run_terminal_command",
                    "output":{"command":"pwd && git status","stdout":"huge\n".repeat(200),"exitCode":0}
                }}),
                None,
            )
            .await
            .expect("complete");
        let text = builder
            .open_text_block
            .as_ref()
            .expect("progress text")
            .1
            .as_str();
        assert!(text.contains("▶ run_terminal_command"));
        assert!(text.contains("✓ run_terminal_command"));
        assert!(!text.contains("exitCode"));
        assert!(!text.contains("huge"));
        assert!(!text.contains("stdout"));
        // Success line is marker-only (no tool body JSON).
        assert!(!text.contains("✓ run_terminal_command:"));
    }

    #[tokio::test]
    async fn renders_update_only_tools_once_and_reuses_their_titles() {
        let mut builder = SegmentBuilder::new(1);
        send_update_statuses(&mut builder).await;
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
        // Progress is accumulated once, then removed from committed output.
        assert_eq!(
            builder
                .open_text_block
                .as_ref()
                .expect("progress text")
                .1
                .matches("▶ WebFetch")
                .count(),
            1
        );
        let segment = builder.finish(None).await.expect("segment");
        assert!(
            segment.blocks[0]["text"]
                .as_str()
                .expect("committed progress")
                .trim()
                .is_empty()
        );
    }

    #[test]
    fn previews_and_truncates_status_output() {
        const UNREACHED_PREVIEW_CHAR_LIMIT: usize = 20;
        const TRUNCATED_PREVIEW_CHAR_LIMIT: usize = 3;
        assert_eq!(failure_preview(Some(&json!("text"))), "text");
        assert_eq!(failure_preview(Some(&json!({"error":"boom\nmore"}))), "boom");
        assert_eq!(failure_preview(None), "failed");
        assert_eq!(compact_title("run_terminal_command: pwd && git status"), "run_terminal_command");
        assert_eq!(
            truncate_for_status("  short  ", UNREACHED_PREVIEW_CHAR_LIMIT),
            "short"
        );
        assert_eq!(
            truncate_for_status("abcdef", TRUNCATED_PREVIEW_CHAR_LIMIT),
            "abc…"
        );
    }

    #[tokio::test]
    async fn counts_only_validated_provider_web_evidence_once() {
        let mut builder = SegmentBuilder::new(1);
        let evidence = json!({
            "provider":"acp",
            "provenance":"provider-native-tool-completion",
            "kind":"web_search",
            "evidence_class":"search_result_only",
            "status":"completed",
            "verified":true,
            "result_summary":"provider search result",
            "source_urls":["https://example.com/result"]
        });
        send_evidence_updates(&mut builder, evidence).await;
        let segment = builder.finish(None).await.expect("segment");
        assert_eq!(segment.usage.web_search_requests, 1);
    }

    async fn collect_frames(
        receiver: &mut mpsc::Receiver<Result<Bytes, Infallible>>,
    ) -> (usize, String) {
        let mut output = String::new();
        let mut frame_count = 0;
        while let Some(frame) = receiver.recv().await {
            frame_count += 1;
            output.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
        }
        (frame_count, output)
    }

    async fn send_update_statuses(builder: &mut SegmentBuilder) {
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
    }

    async fn send_evidence_updates(builder: &mut SegmentBuilder, evidence: Value) {
        for (call_id, evidence) in [
            ("model-prose", json!("https://model.example/prose-url")),
            (
                "missing-source",
                json!({
                    "provider":"acp",
                    "provenance":"provider-native-tool-completion",
                    "kind":"web_search",
                    "evidence_class":"search_result_only",
                    "status":"completed",
                    "verified":true,
                    "result_summary":"missing source URL",
                    "source_urls":[]
                }),
            ),
            ("native-search", evidence.clone()),
            ("native-search", evidence),
        ] {
            builder
                .provider_tool_update(
                    &json!({"params":{
                        "callId":call_id,
                        "status":"completed",
                        "evidence":evidence
                    }}),
                    None,
                )
                .await
                .expect("provider update");
        }
    }

    #[test]
    fn accepts_only_complete_native_web_evidence_with_a_valid_source() {
        let valid = json!({
            "provider":"acp",
            "provenance":"provider-native-tool-completion",
            "kind":"web_search",
            "evidence_class":"search_result_only",
            "status":"completed",
            "verified":true,
            "result_summary":"provider result",
            "source_urls":["https://example.com/result"]
        });
        assert!(validated_provider_web_evidence(Some(&valid)));
        assert!(!validated_provider_web_evidence(None));

        let mut invalid = valid.clone();
        invalid["provenance"] = json!("model-prose");
        assert!(!validated_provider_web_evidence(Some(&invalid)));

        let mut fetch = valid.clone();
        fetch["kind"] = json!("web_fetch");
        fetch["evidence_class"] = json!("fetch_verified");
        fetch["source_urls"] = json!(["http://example.com/fetch"]);
        assert!(validated_provider_web_evidence(Some(&fetch)));

        for (field, value) in [
            ("evidence_class", json!("fetch_verified")),
            ("status", json!("in_progress")),
            ("verified", json!(false)),
            ("result_summary", json!("  ")),
            ("source_urls", json!(["ftp://example.com/result"])),
            ("source_urls", json!([null])),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(!validated_provider_web_evidence(Some(&invalid)));
        }
    }
}
