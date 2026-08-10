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
    assert_eq!(remaining_final_message("hello", "hel"), Some("lo".to_owned()));
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
