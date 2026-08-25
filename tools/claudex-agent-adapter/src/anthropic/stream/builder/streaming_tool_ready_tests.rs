//! Coverage gates measure production tool JSON readiness; this module only contains tests.

use serde_json::json;

use super::{
    ToolJsonReadiness, finished_tool_json_payload, incomplete_tool_json_error,
    requires_complete_tool_json, tool_arguments_ready, tool_input_ready, tool_json_readiness,
};

#[test]
fn empty_bash_payload_is_incomplete() {
    assert!(matches!(
        tool_json_readiness("Bash", ""),
        ToolJsonReadiness::Incomplete
    ));
}

#[test]
fn empty_bash_object_is_incomplete_tool_json() {
    assert!(matches!(
        tool_json_readiness("Bash", "{}"),
        ToolJsonReadiness::Incomplete
    ));
}

#[test]
fn truncated_bash_command_is_truncated_tool_json() {
    assert!(matches!(
        tool_json_readiness("Bash", "{\"command\":\"cat"),
        ToolJsonReadiness::Truncated
    ));
}

#[test]
fn bash_command_json_is_ready() {
    assert!(matches!(
        tool_json_readiness("Bash", r#"{"command":"ls"}"#),
        ToolJsonReadiness::Ready
    ));
}

#[test]
fn empty_send_message_object_is_incomplete_tool_json() {
    assert!(matches!(
        tool_json_readiness("SendMessage", "{}"),
        ToolJsonReadiness::Incomplete
    ));
}

#[test]
fn empty_read_object_is_incomplete_tool_json() {
    assert!(matches!(
        tool_json_readiness("Read", "{}"),
        ToolJsonReadiness::Incomplete
    ));
}

#[test]
fn empty_raw_json_is_incomplete_tool_json() {
    assert!(matches!(
        tool_json_readiness("Read", "   "),
        ToolJsonReadiness::Incomplete
    ));
}

#[test]
fn read_without_file_path_or_path_is_incomplete_tool_json() {
    assert!(matches!(
        tool_json_readiness("Read", "{\"offset\":1}"),
        ToolJsonReadiness::Incomplete
    ));
}

#[test]
fn whitespace_read_path_is_incomplete_tool_json() {
    assert!(matches!(
        tool_json_readiness("Read", "{\"path\":\"  \"}"),
        ToolJsonReadiness::Incomplete
    ));
}

#[test]
fn complete_read_path_is_ready_tool_json() {
    assert!(matches!(
        tool_json_readiness("Read", "{\"path\":\"CLAUDE.md\"}"),
        ToolJsonReadiness::Ready
    ));
}

#[test]
fn complete_read_file_path_is_ready_tool_json() {
    assert!(matches!(
        tool_json_readiness("Read", "{\"file_path\":\"CLAUDE.md\"}"),
        ToolJsonReadiness::Ready
    ));
}

#[test]
fn empty_grep_object_is_incomplete_tool_json() {
    assert!(matches!(
        tool_json_readiness("Grep", "{}"),
        ToolJsonReadiness::Incomplete
    ));
}

#[test]
fn grep_without_pattern_is_incomplete_tool_json() {
    assert!(matches!(
        tool_json_readiness("Grep", "{\"path\":\"src\"}"),
        ToolJsonReadiness::Incomplete
    ));
}

#[test]
fn complete_grep_pattern_is_ready_tool_json() {
    assert!(matches!(
        tool_json_readiness("Grep", "{\"pattern\":\"tool_use\"}"),
        ToolJsonReadiness::Ready
    ));
}

#[test]
fn empty_glob_object_is_incomplete_tool_json() {
    assert!(matches!(
        tool_json_readiness("Glob", "{}"),
        ToolJsonReadiness::Incomplete
    ));
}

#[test]
fn glob_without_pattern_is_incomplete_tool_json() {
    assert!(matches!(
        tool_json_readiness("Glob", "{\"path\":\"src\"}"),
        ToolJsonReadiness::Incomplete
    ));
}

#[test]
fn complete_glob_pattern_is_ready_tool_json() {
    assert!(matches!(
        tool_json_readiness("Glob", "{\"pattern\":\"*.rs\"}"),
        ToolJsonReadiness::Ready
    ));
}

#[test]
fn empty_read_arguments_are_not_ready() {
    assert!(!tool_arguments_ready("Read", &json!({})));
}

#[test]
fn read_offset_only_arguments_are_not_ready() {
    assert!(!tool_arguments_ready("Read", &json!({"offset": 1})));
}

#[test]
fn read_path_arguments_are_ready() {
    assert!(tool_arguments_ready("Read", &json!({"path": "CLAUDE.md"})));
}

#[test]
fn read_file_path_arguments_are_ready() {
    assert!(tool_arguments_ready(
        "Read",
        &json!({"file_path": "CLAUDE.md"})
    ));
}

#[test]
fn empty_grep_arguments_are_not_ready() {
    assert!(!tool_arguments_ready("Grep", &json!({})));
}

#[test]
fn grep_path_without_pattern_is_not_ready() {
    assert!(!tool_arguments_ready("Grep", &json!({"path": "src"})));
}

#[test]
fn grep_pattern_arguments_are_ready() {
    assert!(tool_arguments_ready(
        "Grep",
        &json!({"pattern": "tool_use"})
    ));
}

#[test]
fn empty_glob_arguments_are_not_ready() {
    assert!(!tool_arguments_ready("Glob", &json!({})));
}

#[test]
fn glob_pattern_arguments_are_ready() {
    assert!(tool_arguments_ready("Glob", &json!({"pattern": "*.rs"})));
}

#[test]
fn null_read_arguments_are_not_ready() {
    assert!(!tool_arguments_ready("Read", &json!(null)));
}

#[test]
fn array_grep_arguments_are_not_ready() {
    assert!(!tool_arguments_ready("Grep", &json!([])));
}

#[test]
fn web_tools_require_their_complete_required_arguments() {
    assert!(!tool_arguments_ready("WebSearch", &json!({})));
    assert!(!tool_arguments_ready("WebSearch", &json!({"query": "  "})));
    assert!(tool_arguments_ready(
        "WebSearch",
        &json!({"query": "AVITA"})
    ));
    assert!(!tool_arguments_ready(
        "WebFetch",
        &json!({"url": "https://avita.co.jp"})
    ));
    assert!(!tool_arguments_ready(
        "WebFetch",
        &json!({"prompt": "extract facts"})
    ));
    assert!(tool_arguments_ready(
        "WebFetch",
        &json!({"url": "https://avita.co.jp", "prompt": "extract facts"})
    ));
}

#[test]
fn web_tools_require_complete_json_and_report_specific_errors() {
    for tool in ["WebSearch", "WebFetch"] {
        assert!(requires_complete_tool_json(tool));
        assert!(matches!(
            tool_json_readiness(tool, "{}"),
            ToolJsonReadiness::Incomplete
        ));
    }
    assert_eq!(
        incomplete_tool_json_error("WebSearch"),
        "Incomplete WebSearch tool JSON was not flushed; a non-empty query is required."
    );
    assert_eq!(
        incomplete_tool_json_error("WebFetch"),
        "Incomplete WebFetch tool JSON was not flushed; non-empty url and prompt are required."
    );
}

#[test]
fn write_empty_object_stays_ready() {
    assert!(tool_arguments_ready("Write", &json!({})));
}

#[test]
fn read_requires_complete_json() {
    assert!(requires_complete_tool_json("Read"));
}

#[test]
fn lowercase_read_requires_complete_json() {
    assert!(requires_complete_tool_json("read"));
}

#[test]
fn grep_requires_complete_json() {
    assert!(requires_complete_tool_json("Grep"));
}

#[test]
fn glob_requires_complete_json() {
    assert!(requires_complete_tool_json("Glob"));
}

#[test]
fn bash_requires_complete_json() {
    assert!(requires_complete_tool_json("Bash"));
}

#[test]
fn send_message_requires_complete_json() {
    assert!(requires_complete_tool_json("SendMessage"));
}

#[test]
fn write_does_not_require_complete_json() {
    assert!(!requires_complete_tool_json("Write"));
}

#[test]
fn incomplete_read_error_requires_file_path_or_path() {
    assert_eq!(
        incomplete_tool_json_error("Read"),
        "Incomplete Read tool JSON was not flushed; a non-empty file_path or path is required."
    );
}

#[test]
fn incomplete_grep_error_requires_pattern() {
    assert_eq!(
        incomplete_tool_json_error("Grep"),
        "Incomplete Grep tool JSON was not flushed; a non-empty pattern is required."
    );
}

#[test]
fn incomplete_glob_error_requires_pattern() {
    assert_eq!(
        incomplete_tool_json_error("Glob"),
        "Incomplete Glob tool JSON was not flushed; a non-empty pattern is required."
    );
}

#[test]
fn incomplete_bash_error_requires_command() {
    assert_eq!(
        incomplete_tool_json_error("Bash"),
        "Incomplete Bash tool JSON was not flushed; a non-empty command is required."
    );
}

#[test]
fn incomplete_send_message_error_requires_to_and_message() {
    assert_eq!(
        incomplete_tool_json_error("SendMessage"),
        "Incomplete SendMessage tool JSON was not flushed; non-empty to and message are required."
    );
}

#[test]
fn incomplete_write_error_uses_generic_missing_keys() {
    assert_eq!(
        incomplete_tool_json_error("Write"),
        "Incomplete tool JSON was not flushed; required keys are missing."
    );
}

#[test]
fn tool_input_ready_rejects_empty_read_object() {
    assert!(!tool_input_ready("Read", "{}", &json!({})));
}

#[test]
fn tool_input_ready_accepts_complete_read_arguments() {
    assert!(tool_input_ready(
        "Read",
        "{}",
        &json!({"path": "CLAUDE.md"})
    ));
}

#[test]
fn tool_input_ready_accepts_complete_read_partial_json() {
    assert!(tool_input_ready(
        "Read",
        "{\"file_path\":\"CLAUDE.md\"}",
        &json!({})
    ));
}

#[test]
fn finished_payload_uses_ready_read_arguments() {
    assert_eq!(
        finished_tool_json_payload("Read", "{}", &json!({"path": "CLAUDE.md"})),
        "{\"path\":\"CLAUDE.md\"}"
    );
}

#[test]
fn finished_payload_keeps_held_read_partial_json_when_arguments_are_empty() {
    assert_eq!(
        finished_tool_json_payload("Read", "{\"path\":\"held.md\"}", &json!({})),
        "{\"path\":\"held.md\"}"
    );
}
