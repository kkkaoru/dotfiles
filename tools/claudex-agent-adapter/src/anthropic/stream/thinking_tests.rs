#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::convert::Infallible;

    use axum::body::Bytes;

    use super::*;

    #[tokio::test]
    async fn heartbeat_closes_empty_non_keepalive_open_before_emitting() {
        let mut state = ThinkingState {
            open: Some(OpenThinking {
                index: 0,
                item_id: "reasoning".to_owned(),
                summary_index: 0,
                signature: "sig".to_owned(),
                text: String::new(),
            }),
        };
        let mut blocks = vec![json!({"type":"thinking","thinking":"","signature":""})];
        state
            .activity_keepalive(&mut blocks, None)
            .await
            .expect("close empty reasoning without opening status thinking");
        assert_eq!(blocks.len(), 1);
        assert!(
            state.open.is_none(),
            "empty CoT must close; silence must not open STATUS thinking"
        );
    }

    #[tokio::test]
    async fn heartbeat_keeps_non_empty_reasoning_open_and_appends_zwsp() {
        let mut state = ThinkingState {
            open: Some(OpenThinking {
                index: 0,
                item_id: "reasoning".to_owned(),
                summary_index: 0,
                signature: "sig".to_owned(),
                text: "already thinking".to_owned(),
            }),
        };
        let mut blocks = vec![json!({
            "type":"thinking",
            "thinking":"already thinking",
            "signature":"sig"
        })];
        state
            .activity_keepalive(&mut blocks, None)
            .await
            .expect("keep reasoning open");
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            state.open.as_ref().map(|open| open.item_id.as_str()),
            Some("reasoning")
        );
        assert_eq!(
            state.open.as_ref().map(|open| open.text.as_str()),
            Some("already thinking"),
            "CoT buffer stays clean; keepalive ZWSP is stream-only"
        );
    }

    #[tokio::test]
    async fn coalesced_delta_promotes_keepalive_chrome_to_native_thought() {
        let mut state = ThinkingState {
            open: Some(OpenThinking {
                index: 0,
                item_id: "claudex_activity_keepalive".to_owned(),
                summary_index: 0,
                signature: thinking_signature("claudex_activity_keepalive"),
                text: String::new(),
            }),
        };
        let mut blocks = vec![json!({"type":"thinking","thinking":"","signature":""})];
        state
            .delta_text_coalesced("reasoning", 0, "promoted", &mut blocks, None)
            .await
            .expect("promote keepalive");
        assert_eq!(
            state.open.as_ref().map(|open| open.item_id.as_str()),
            Some("reasoning")
        );
        assert!(
            state
                .open
                .as_ref()
                .is_some_and(|open| open.text.contains("promoted"))
        );
    }

    #[tokio::test]
    async fn progress_status_dedup_with_trailing_newline_after_rewrite() {
        // Edge case: after strip_worker_status_lines, buffer may end with \n.
        // Appending same status again should dedup, not double-emit.
        let mut state = ThinkingState {
            open: Some(OpenThinking {
                index: 0,
                item_id: "claudex_provider_progress".to_owned(),
                summary_index: 0,
                signature: "sig".to_owned(),
                text: "Status: old line\n".to_owned(), // trailing \n from rewrite
            }),
        };
        let mut blocks = vec![json!({
            "type":"thinking",
            "thinking":"Status: old line\n",
            "signature":"sig"
        })];
        // Now append the same status (simulating replace_live_worker_status).
        let status = "Status: old line\n"; // same status with newline
        state
            .progress_status_on(&mut blocks, status, true, None)
            .await
            .expect("dedup with trailing newline");
        // Verify text didn't duplicate.
        let text = state.open.as_ref().map(|open| open.text.as_str());
        assert_eq!(
            text,
            Some("Status: old line\n"),
            "duplicate status with trailing newline should be deduplicated"
        );
    }

    #[tokio::test]
    async fn elapsed_keepalive_is_stream_only_so_tip_stays_last_visible() {
        use axum::body::Bytes;
        use std::{convert::Infallible, time::Duration};
        use tokio::sync::mpsc;

        let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(8);
        let mut state = ThinkingState::default();
        let mut blocks = Vec::new();
        let tip = "▶ Read\n";
        state
            .progress_status_keep_open(&mut blocks, tip, Some(&sender))
            .await
            .expect("tip");
        let before = blocks[0]["thinking"].as_str().expect("tip text").to_owned();
        state
            .elapsed_keepalive(&blocks, Duration::from_secs(4), Some("Read"), Some(&sender))
            .await
            .expect("zwsp");
        assert_eq!(
            blocks[0]["thinking"].as_str(),
            Some(before.as_str()),
            "keepalive must not park ZWSP in the tip buffer"
        );
        state
            .progress_status_keep_open(&mut blocks, tip, Some(&sender))
            .await
            .expect("deduped re-tip");
        assert_eq!(
            blocks[0]["thinking"]
                .as_str()
                .expect("thinking")
                .matches("▶ Read")
                .count(),
            1
        );
        drop(sender);
        let (tip_deltas, zwsp_deltas) = count_tip_and_zwsp_deltas(&mut receiver).await;
        assert_eq!(tip_deltas, 1);
        assert_eq!(zwsp_deltas, 1);
    }

    #[tokio::test]
    async fn closes_effort_launch_thought_before_progress_marker() {
        let mut state = ThinkingState {
            open: Some(OpenThinking {
                index: 0,
                item_id: "claudex_activity_keepalive".to_owned(),
                summary_index: 0,
                signature: "sig".to_owned(),
                text: "SubAgent starting: auto (effort=high); preparing provider session…"
                    .to_owned(),
            }),
        };
        let mut blocks = vec![json!({
            "type":"thinking",
            "thinking":"SubAgent starting: auto (effort=high); preparing provider session…",
            "signature":"sig"
        })];
        assert!(state.open_holds_collapsed_subagent_launch());
        state.close(&mut blocks, None).await.expect("close");
        assert!(state.open.is_none(), "launch prose must close before ▶");
        state
            .progress_status_keep_open(&mut blocks, "▶ Bash\n", None)
            .await
            .expect("fresh ▶");
        assert_eq!(
            state.open.as_ref().map(|open| open.item_id.as_str()),
            Some("claudex_provider_progress")
        );
        assert!(
            marker_outside_wandering_launch(&blocks),
            "▶ must open outside the Wandering launch block: {blocks:?}"
        );
    }

    #[test]
    fn zwsp_and_thinking_elapsed_tip_count_as_collapsed_prime() {
        let mut state = ThinkingState {
            open: Some(OpenThinking {
                index: 0,
                item_id: "claudex_activity_keepalive".to_owned(),
                summary_index: 0,
                signature: "sig".to_owned(),
                text: HEARTBEAT.to_owned(),
            }),
        };
        assert!(state.open_holds_collapsed_subagent_launch());
        assert!(state.open_holds_zwsp_or_launch_prose());
        state.open.as_mut().expect("open").text = format!("{HEARTBEAT}▶ Thinking… · 0s\n");
        assert!(
            state.open_holds_collapsed_subagent_launch(),
            "ZWSP + ▶ Thinking… · 0s must close before live chrome"
        );
        assert!(
            !state.open_holds_zwsp_or_launch_prose(),
            "keepalive must not close the elapsed Thinking tip every tick"
        );
        state.open.as_mut().expect("open").text = "▶ Read CLAUDE.md\n".to_owned();
        assert!(!state.open_holds_collapsed_subagent_launch());
        state.open.as_mut().expect("open").item_id = "claudex_provider_progress".to_owned();
        state.open.as_mut().expect("open").text = "▶ Thinking… · 0s\n".to_owned();
        assert!(state.open_holds_collapsed_subagent_launch());
        state.open.as_mut().expect("open").text =
            "Nucleating… (↓ tokens · still thinking with high effort)".to_owned();
        assert!(state.open_holds_zwsp_or_launch_prose());
        assert!(state.open_holds_collapsed_subagent_launch());
    }

    #[tokio::test]
    async fn close_before_executable_tool_use_promotes_keepalive_progress() {
        let mut state = ThinkingState {
            open: Some(OpenThinking {
                index: 0,
                item_id: "claudex_provider_progress".to_owned(),
                summary_index: 0,
                signature: "sig".to_owned(),
                text: "▶ Thinking… · 1s\n".to_owned(),
            }),
        };
        let mut blocks = vec![json!({
            "type":"thinking",
            "thinking":"▶ Thinking… · 1s\n",
            "signature":"sig"
        })];
        state
            .close_before_executable_tool_use(&mut blocks, None)
            .await
            .expect("close keepalive before tool_use");
        assert!(state.open.is_none());
        assert_eq!(blocks[0]["thinking"], json!("▶ Thinking… · 1s\n"));
        assert_ne!(blocks[0]["signature"], json!("sig"));
    }

    #[test]
    fn thinking_elapsed_chrome_detects_only_thinking_tips() {
        use super::super::thinking_support::is_thinking_elapsed_chrome;
        assert!(is_thinking_elapsed_chrome("▶ Thinking… · 0s\n"));
        assert!(is_thinking_elapsed_chrome(
            "▶ Thinking… · 0s\n▶ Thinking… · 1s\n"
        ));
        assert!(!is_thinking_elapsed_chrome(
            "Anchor the Avita research next.\n▶ Thinking… · 0s\n"
        ));
        assert!(!is_thinking_elapsed_chrome("▶ Read · 4s\n"));
    }

    #[tokio::test]
    async fn keep_open_does_not_nest_thinking_elapsed_chrome_inside_cot() {
        let mut state = ThinkingState::default();
        let mut blocks = Vec::new();
        state
            .progress_status_keep_open(&mut blocks, "Anchor the Avita research next.\n", None)
            .await
            .expect("cot");
        state
            .progress_status_keep_open(&mut blocks, "▶ Thinking… · 0s\n", None)
            .await
            .expect("nested 0s");
        state
            .progress_status_keep_open(&mut blocks, "▶ Thinking… · 1s\n", None)
            .await
            .expect("nested 1s");
        let text = &state.open.as_ref().expect("open").text;
        assert!(
            text.contains("Anchor the Avita"),
            "CoT must stay visible: {text:?}"
        );
        assert!(
            !text.contains("▶ Thinking"),
            "thinking nested inside thinking: {text:?}"
        );
    }

    #[tokio::test]
    async fn keep_open_does_not_stack_thinking_elapsed_clocks() {
        let mut state = ThinkingState::default();
        let mut blocks = Vec::new();
        state
            .progress_status_keep_open(&mut blocks, "▶ Thinking… · 0s\n", None)
            .await
            .expect("0s");
        state
            .progress_status_keep_open(&mut blocks, "▶ Thinking… · 1s\n", None)
            .await
            .expect("1s");
        let text = state
            .open
            .as_ref()
            .map(|open| open.text.as_str())
            .unwrap_or("");
        assert!(
            !text.contains("▶ Thinking"),
            "thinking elapsed chrome must not land in thinking: {text:?}"
        );
        assert!(
            !(text.contains("· 0s") && text.contains("· 1s")),
            "stacked thinking clocks: {text:?}"
        );
    }

    #[tokio::test]
    async fn commit_buffered_reasoning_ignores_empty_and_opens_when_closed() {
        let mut state = ThinkingState::default();
        let mut blocks = Vec::new();
        state
            .commit_buffered_reasoning(&mut blocks, "subagent:reasoning", "   ", None)
            .await
            .expect("empty reasoning is a no-op");
        assert!(blocks.is_empty());
        state
            .commit_buffered_reasoning(&mut blocks, "subagent:reasoning", "visible CoT", None)
            .await
            .expect("open a thinking block for CoT");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["thinking"], json!("visible CoT"));
    }

    fn marker_outside_wandering_launch(blocks: &[Value]) -> bool {
        let Some(text) = blocks
            .last()
            .and_then(|block| block.get("thinking"))
            .and_then(Value::as_str)
        else {
            return false;
        };
        text.contains('▶') && !text.contains("effort=")
    }

    async fn count_tip_and_zwsp_deltas(
        receiver: &mut tokio::sync::mpsc::Receiver<Result<Bytes, Infallible>>,
    ) -> (usize, usize) {
        let mut tip_deltas = 0usize;
        let mut zwsp_deltas = 0usize;
        while let Some(frame) = receiver.recv().await {
            classify_thinking_delta_frame(
                &frame.expect("frame"),
                &mut tip_deltas,
                &mut zwsp_deltas,
            );
        }
        (tip_deltas, zwsp_deltas)
    }

    fn classify_thinking_delta_frame(
        frame: &Bytes,
        tip_deltas: &mut usize,
        zwsp_deltas: &mut usize,
    ) {
        let frame = String::from_utf8(frame.to_vec()).expect("utf8");
        let data = frame.lines().find_map(|line| line.strip_prefix("data: "));
        let value = serde_json::from_str::<Value>(data.expect("data")).expect("json");
        match value.pointer("/delta/thinking").and_then(Value::as_str) {
            Some(text) if text.contains("▶ Read") => *tip_deltas += 1,
            Some("​") => *zwsp_deltas += 1,
            _ => {}
        }
    }

    #[test]
    fn promote_keepalive_progress_covers_missing_open_and_foreign_item_ids() {
        let mut empty = ThinkingState::default();
        empty.promote_keepalive_progress("reasoning");
        assert!(empty.open.is_none());

        let mut foreign = ThinkingState {
            open: Some(OpenThinking {
                index: 0,
                item_id: "reasoning".to_owned(),
                summary_index: 0,
                signature: "sig".to_owned(),
                text: "kept".to_owned(),
            }),
        };
        foreign.promote_keepalive_progress("subagent:reasoning");
        assert_eq!(
            foreign.open.as_ref().map(|open| open.item_id.as_str()),
            Some("reasoning")
        );
    }

    #[tokio::test]
    async fn ensure_open_is_a_no_op_when_thinking_is_already_open() {
        let mut state = ThinkingState::default();
        let mut blocks = Vec::new();
        state
            .ensure_open(&mut blocks, None)
            .await
            .expect("first ensure_open");
        let first_id = state.open.as_ref().map(|open| open.item_id.clone());
        state
            .ensure_open(&mut blocks, None)
            .await
            .expect("second ensure_open");
        assert_eq!(
            state.open.as_ref().map(|open| open.item_id.as_str()),
            first_id.as_deref()
        );
        assert_eq!(blocks.len(), 1);
    }

    #[tokio::test]
    async fn progress_status_trims_whitespace_and_skips_zwsp_only_status() {
        let mut state = ThinkingState::default();
        let mut blocks = Vec::new();
        state
            .progress_status_keep_open(&mut blocks, "hello ", None)
            .await
            .expect("trailing space status");
        state
            .progress_status_keep_open(&mut blocks, "world", None)
            .await
            .expect("buffer had trailing space");
        state
            .progress_status_keep_open(&mut blocks, "\u{200b}", None)
            .await
            .expect("zwsp-only status");
        let text = state.open.as_ref().map(|open| open.text.as_str());
        assert_eq!(text, Some("hello world\u{200b}"));
    }

    fn drain_live_frames(receiver: &mut tokio::sync::mpsc::Receiver<Result<Bytes, Infallible>>) -> String {
        let mut live = String::new();
        while let Ok(frame) = receiver.try_recv() {
            live.push_str(&String::from_utf8_lossy(&frame.expect("frame")));
        }
        live
    }

    fn committed_thinking(blocks: &[serde_json::Value]) -> Vec<String> {
        blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("thinking"))
            .map(|block| {
                block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            })
            .collect()
    }

    #[tokio::test]
    async fn t2_live_tip_then_cot_commits_reasoning_only() {
        use tokio::sync::mpsc;
        let (sender, mut receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
        let mut state = ThinkingState::default();
        let mut blocks = Vec::new();
        state
            .progress_status_keep_open(&mut blocks, "▶ Read src/lib.rs\n", Some(&sender))
            .await
            .expect("live tip");
        state
            .delta_text_coalesced("reasoning", 0, "Need to inspect the module graph.", &mut blocks, None)
            .await
            .expect("real CoT");
        drop(sender);
        let live = drain_live_frames(&mut receiver);
        assert!(live.contains('▶'), "live SSE must show ▶: {live}");
        crate::anthropic::stream::sanitize::sanitize_committed_blocks(&mut blocks);
        let thinking = committed_thinking(&blocks);
        assert_eq!(thinking.len(), 1, "{thinking:?}");
        assert!(
            thinking[0].contains("Need to inspect the module graph"),
            "{thinking:?}"
        );
        assert!(!thinking[0].contains('▶'), "{thinking:?}");
        assert!(!thinking[0].contains("Claudex is still working"), "{thinking:?}");
    }
}
