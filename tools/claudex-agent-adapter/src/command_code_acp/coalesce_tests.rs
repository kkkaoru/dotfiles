use super::*;

#[test]
fn remaining_final_message_skips_when_streamed_already_covers_result() {
    assert_eq!(remaining_final_message("", "streamed"), None);
    assert_eq!(
        remaining_final_message("hello world", ""),
        Some("hello world".to_owned())
    );
    assert_eq!(remaining_final_message("hello", "hello"), None);
    assert_eq!(remaining_final_message("hello", "hello world"), None);
    assert_eq!(
        remaining_final_message("hello", "hel"),
        Some("lo".to_owned())
    );
    assert_eq!(remaining_final_message("hello", "hello   "), None);
    assert_eq!(
        remaining_final_message("fresh answer", "old"),
        Some("fresh answer".to_owned())
    );
}

#[test]
fn should_flush_rejects_empty_buffers_and_honors_punctuation() {
    assert!(!should_flush("", 1));
    assert!(should_flush("abcdefgh", 8));
    assert!(should_flush("line\n", 1));
    assert!(should_flush("done.", 1));
    assert!(should_flush("wow!", 1));
    assert!(should_flush("huh?", 1));
    assert!(should_flush("完了。", 1));
    assert!(should_flush("すごい！", 1));
    assert!(should_flush("本当？", 1));
    assert!(should_flush(&"あ".repeat(80), 1));
    assert!(!should_flush("ab", 2));
}

#[test]
fn thinking_end_snapshot_lands_when_muse_skipped_deltas() {
    let mut coalescer = ProgressCoalescer::default();
    assert_eq!(
        coalescer.push(ProgressEvent::ThoughtEnd(
            "Outline the Muse migration before tools.".to_owned()
        )),
        vec![ProgressEvent::Thought(
            "Outline the Muse migration before tools.".to_owned()
        )]
    );
}

#[test]
fn thinking_end_snapshot_is_ignored_after_streamed_deltas() {
    let mut coalescer = ProgressCoalescer::default();
    assert_eq!(
        coalescer.push(ProgressEvent::Thought("planning live.".to_owned())),
        vec![ProgressEvent::Thought("planning live.".to_owned())]
    );
    assert!(
        coalescer
            .push(ProgressEvent::ThoughtEnd(
                "planning live. full snapshot replay".to_owned()
            ))
            .is_empty(),
        "replaying Muse thinking_end after deltas must not reopen Thought-for chrome"
    );
}

#[test]
fn thinking_end_after_tool_can_start_a_new_unit() {
    let mut coalescer = ProgressCoalescer::default();
    assert_eq!(
        coalescer.push(ProgressEvent::Thought("before tool.".to_owned())),
        vec![ProgressEvent::Thought("before tool.".to_owned())]
    );
    let flushed = coalescer.push(ProgressEvent::ToolStarted {
        id: "t1".to_owned(),
        name: "Read".to_owned(),
        description: None,
    });
    assert!(flushed.iter().any(|event| matches!(
        event,
        ProgressEvent::ToolStarted { name, .. } if name == "Read"
    )));
    assert_eq!(
        coalescer.push(ProgressEvent::ThoughtEnd(
            "after tool, delta-less snapshot".to_owned()
        )),
        vec![ProgressEvent::Thought(
            "after tool, delta-less snapshot".to_owned()
        )]
    );
}
