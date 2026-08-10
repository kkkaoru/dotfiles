#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
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
            .expect("close empty reasoning then open keepalive");
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            state.open.as_ref().map(|open| open.item_id.as_str()),
            Some("claudex_activity_keepalive")
        );
        assert!(
            state
                .open
                .as_ref()
                .is_some_and(|open| open.text.contains("still working")),
            "first keepalive should be the visible status"
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
        assert!(
            state
                .open
                .as_ref()
                .is_some_and(|open| open.text.ends_with(HEARTBEAT)),
            "non-empty reasoning should receive ZWSP heartbeat"
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
}
