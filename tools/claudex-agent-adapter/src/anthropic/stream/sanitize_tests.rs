use super::strip_worker_status_lines;

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
