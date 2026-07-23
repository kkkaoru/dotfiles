#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use axum::body::Bytes;
    use serde_json::json;
    use std::convert::Infallible;
    use tokio::sync::mpsc;

    use super::*;

    #[tokio::test]
    async fn builds_provider_cards_and_reports_malformed_calls() {
        let mut builder = SegmentBuilder::new(1);
        assert!(builder.provider_tool_call(&json!({}), None).await.is_err());
        assert!(
            builder
                .provider_tool_call(&json!({"params":{}}), None)
                .await
                .is_err()
        );
        builder
            .provider_tool_call(
                &json!({"params":{"callId":"agent:1","tool":"Read","arguments":{"path":"a"}}}),
                None,
            )
            .await
            .expect("provider card");
        builder
            .provider_tool_call(&json!({"params":{"callId":"2"}}), None)
            .await
            .expect("default provider card");

        assert_eq!(builder.blocks[0]["id"], "toolu_provider_agent_1");
        assert_eq!(builder.blocks[0]["name"], "Read");
        assert_eq!(builder.blocks[0]["input"]["path"], "a");
        assert_eq!(builder.blocks[1]["name"], "Tool");
        assert_eq!(builder.provider_tool_ids.len(), 2);
    }

    #[tokio::test]
    async fn streams_provider_cards_and_all_status_variants() {
        let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(32);
        let mut builder = SegmentBuilder::new(1);
        builder
            .provider_tool_call(
                &json!({"params":{"callId":"1","tool":"Bash","arguments":{}}}),
                Some(&sender),
            )
            .await
            .expect("stream card");
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
        builder
            .append_text("", Some(&sender))
            .await
            .expect("empty text");
        assert!(
            builder
                .provider_tool_update(&json!({}), None)
                .await
                .is_err()
        );
        let segment = builder.finish(Some(&sender)).await.expect("segment");
        drop(sender);

        let text = segment.blocks[1]["text"].as_str().expect("status text");
        assert!(text.contains("✗ Build: {\"code\":1}"));
        assert!(text.contains("✓ Read: done"));
        let mut frame_count = 0;
        while receiver.recv().await.is_some() {
            frame_count += 1;
        }
        assert!(frame_count >= 6);
    }

    #[test]
    fn previews_and_truncates_status_output() {
        assert_eq!(output_preview(Some(&json!("text")), "fallback"), "text");
        assert_eq!(
            output_preview(Some(&json!({"a":1})), "fallback"),
            "{\"a\":1}"
        );
        assert_eq!(output_preview(None, "fallback"), "fallback");
        assert_eq!(truncate_for_status("  short  ", 20), "short");
        assert_eq!(truncate_for_status("abcdef", 3), "abc…");
    }
}
