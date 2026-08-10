use super::{Utf8LineDecoder, fallback_after_stdout};
use std::io;

#[test]
fn decoder_replaces_invalid_bytes_and_keeps_following_json() {
    let mut decoder = Utf8LineDecoder::default();
    assert_eq!(decoder.push_line(b"\xff\n"), "\u{FFFD}");
    let result = decoder.push_line(
        br#"{"type":"result","subtype":"success","finalText":"AFTER_INVALID_UTF8"}"#,
    );
    assert!(result.contains("AFTER_INVALID_UTF8"));
    assert!(!decoder.has_pending());
}

#[test]
fn decoder_carries_incomplete_utf8_across_reads() {
    let mut decoder = Utf8LineDecoder::default();
    assert!(decoder.push_line(&[0xe3]).is_empty());
    assert!(decoder.has_pending());
    assert_eq!(decoder.push_line(&[0x81, 0x82, b'\n']), "あ");
    assert!(!decoder.has_pending());
}

#[test]
fn decoder_flush_lossy_decodes_trailing_incomplete_utf8() {
    let mut decoder = Utf8LineDecoder::default();
    assert!(decoder.push_line(&[0xe3]).is_empty());
    assert_eq!(decoder.flush().as_deref(), Some("\u{FFFD}"));
    assert!(!decoder.has_pending());
}

#[test]
fn decoder_strips_crlf_and_fallback_includes_stderr_on_stdout_error() {
    let mut decoder = Utf8LineDecoder::default();
    assert_eq!(decoder.push_line(b"hello\r\n"), "hello");
    assert_eq!(decoder.push_line(b"world\r"), "world");

    let with_stderr = fallback_after_stdout(
        Some(1),
        false,
        "boom details",
        Some(&io::Error::other("pipe closed")),
    );
    assert_eq!(with_stderr.subtype, "error");
    assert!(
        with_stderr
            .error
            .as_deref()
            .is_some_and(|msg| msg.contains("pipe closed") && msg.contains("boom details"))
    );

    let without_stderr = fallback_after_stdout(None, false, "", Some(&io::Error::other("eof")));
    assert!(
        without_stderr
            .error
            .as_deref()
            .is_some_and(|msg| msg.contains("eof") && msg.contains("terminated"))
    );
}
