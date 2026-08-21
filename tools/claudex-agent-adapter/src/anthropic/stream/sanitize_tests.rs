use serde_json::json;

use super::{is_visible_activity_event, sanitize_committed_blocks, strip_worker_status_lines};

#[test]
fn contentless_reasoning_progress_counts_as_provider_activity() {
    assert!(is_visible_activity_event(
        &json!({"method":"item/reasoning/progress","params":{"threadId":"thread"}})
    ));
}

#[test]
fn drops_unsigned_and_adapter_local_thinking_from_committed_blocks() {
    let mut blocks = vec![
        json!({"type":"thinking","thinking":"unsigned draft","signature":""}),
        json!({
            "type":"thinking",
            "thinking":"adapter invented",
            "signature":"claudex_local_abc123"
        }),
        json!({
            "type":"thinking",
            "thinking":"keepalive",
            "signature":"claudex_activity_keepalive"
        }),
        json!({
            "type":"thinking",
            "thinking":"provider progress",
            "signature":"claudex_provider_progress"
        }),
        json!({
            "type":"thinking",
            "thinking":"keep this reasoning",
            "signature":"grok_provider_sig"
        }),
        json!({"type":"text","text":"visible answer"}),
    ];

    sanitize_committed_blocks(&mut blocks);

    assert_eq!(
        blocks,
        vec![
            json!({
                "type":"thinking",
                "thinking":"keep this reasoning",
                "signature":"grok_provider_sig"
            }),
            json!({"type":"text","text":"visible answer"}),
        ]
    );
}

#[test]
fn strip_worker_status_lines_removes_single_status_line() {
    let input = "Status: inspecting files\n";
    assert_eq!(strip_worker_status_lines(input), "");
}

#[test]
fn strip_worker_status_lines_preserves_non_status_content() {
    let input = "Thought for 2s\nStatus: done\nResult: success\n";
    assert_eq!(
        strip_worker_status_lines(input),
        "Thought for 2s\nResult: success\n"
    );
}

#[test]
fn strip_worker_status_lines_case_insensitive() {
    let input = "status: foo\nSTATUS: bar\nStatus: baz\nother";
    assert_eq!(strip_worker_status_lines(input), "other");
}

#[test]
fn strip_worker_status_lines_preserves_indentation() {
    let input = "  Status: indented\nRegular line";
    assert_eq!(strip_worker_status_lines(input), "Regular line");
}

#[test]
fn strip_worker_status_lines_preserves_trailing_newline() {
    let input = "Line 1\nStatus: removed\nLine 2\n";
    assert_eq!(strip_worker_status_lines(input), "Line 1\nLine 2\n");
}

#[test]
fn strip_worker_status_lines_all_empty_after_strip() {
    let input = "Status: only\n  \nstatus: here";
    assert_eq!(strip_worker_status_lines(input), "");
}
