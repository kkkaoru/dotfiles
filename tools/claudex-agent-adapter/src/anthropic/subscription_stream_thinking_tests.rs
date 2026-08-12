use super::*;
use crate::anthropic::official_claude_haiku_model;

#[tokio::test]
async fn forwards_claude_native_thinking_for_every_subscription_model() {
    for model in [
        official_claude_haiku_model(),
        "claude-sonnet-5",
        "claude-opus-5",
        "claude-opus-5[1m]",
        "claude-haiku-4-5",
    ] {
        let (sender, mut receiver) = channel();
        let mut stream = bare_subscription_stream(Vec::new());
        for line in [
            r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Plan the fix."}}}"#,
            r#"{"type":"stream_event","event":{"delta":{"type":"thinking_delta","thinking":" Check tests."}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-claude"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_stop","index":0}}"#,
            r#"{"type":"stream_event","event":{"delta":{"type":"text_delta","text":"Done."}}}"#,
        ] {
            stream
                .handle_line(&sender, line)
                .await
                .unwrap_or_else(|error| panic!("{model}: {error:#}"));
        }
        let frames = output(&mut receiver).await;
        assert!(
            frames.contains(r#""type":"thinking""#),
            "{model}: missing thinking block start: {frames}"
        );
        assert!(
            frames.contains("Plan the fix.") && frames.contains(" Check tests."),
            "{model}: missing thinking deltas: {frames}"
        );
        assert!(
            frames.contains(r#""type":"signature_delta""#) && frames.contains("sig-claude"),
            "{model}: missing thinking signature: {frames}"
        );
        assert!(
            frames.contains(r#""text":"Done.""#),
            "{model}: missing text after thinking: {frames}"
        );
        let (block_types, _) = collect_block_events(&frames);
        assert_eq!(
            block_types,
            vec![(0, "thinking".to_owned()), (1, "text".to_owned())],
            "{model}: unexpected block order: {block_types:?} in {frames}"
        );
    }
}

#[tokio::test]
async fn abbreviated_thinking_delta_still_opens_a_claude_thinking_block() {
    let (sender, mut receiver) = channel();
    let mut stream = bare_subscription_stream(Vec::new());
    stream
        .handle_line(
            &sender,
            r#"{"type":"stream_event","event":{"delta":{"type":"thinking_delta","thinking":"raw cot"}}}"#,
        )
        .await
        .expect("abbreviated thinking delta");
    let frames = output(&mut receiver).await;
    assert!(frames.contains(r#""type":"thinking""#));
    assert!(frames.contains("raw cot"));
    assert!(frames.contains("thinking_delta"));
}
