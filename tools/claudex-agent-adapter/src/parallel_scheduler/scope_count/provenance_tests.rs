use serde_json::Value;

use super::{
    independent_scope_count,
    test_support::{
        fake_meta_block, fake_meta_message, messages_request, real_three_scope_message,
    },
};

#[test]
fn message_is_meta_provenance_is_ignored() {
    let request = messages_request(vec![
        real_three_scope_message(),
        fake_meta_message("isMeta", Value::Bool(true)),
    ]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn message_is_meta_snake_case_provenance_is_ignored() {
    let request = messages_request(vec![
        real_three_scope_message(),
        fake_meta_message("is_meta", Value::Bool(true)),
    ]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn message_source_tool_use_id_provenance_is_ignored() {
    let request = messages_request(vec![
        real_three_scope_message(),
        fake_meta_message("sourceToolUseID", Value::String("tool".into())),
    ]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn message_source_tool_use_id_snake_case_provenance_is_ignored() {
    let request = messages_request(vec![
        real_three_scope_message(),
        fake_meta_message("source_tool_use_id", Value::String("tool".into())),
    ]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn message_attribution_skill_provenance_is_ignored() {
    let request = messages_request(vec![
        real_three_scope_message(),
        fake_meta_message("attributionSkill", Value::String("loop".into())),
    ]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn block_is_meta_provenance_is_ignored() {
    let request = messages_request(vec![
        real_three_scope_message(),
        fake_meta_block("isMeta", Value::Bool(true)),
    ]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn block_is_meta_snake_case_provenance_is_ignored() {
    let request = messages_request(vec![
        real_three_scope_message(),
        fake_meta_block("is_meta", Value::Bool(true)),
    ]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn block_source_tool_use_id_provenance_is_ignored() {
    let request = messages_request(vec![
        real_three_scope_message(),
        fake_meta_block("sourceToolUseID", Value::String("tool".into())),
    ]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn block_source_tool_use_id_snake_case_provenance_is_ignored() {
    let request = messages_request(vec![
        real_three_scope_message(),
        fake_meta_block("source_tool_use_id", Value::String("tool".into())),
    ]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn block_attribution_skill_provenance_is_ignored() {
    let request = messages_request(vec![
        real_three_scope_message(),
        fake_meta_block("attributionSkill", Value::String("loop".into())),
    ]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn false_and_null_provenance_do_not_hide_real_text() {
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "isMeta":false,
        "sourceToolUseID":null,
        "content":[{
            "type":"text",
            "is_meta":false,
            "attributionSkill":null,
            "text":"Tasks:\n- implement parser\n- verify renderer\n- test integration"
        }]
    })]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn attachment_provenance_is_ignored() {
    let request = messages_request(vec![
        real_three_scope_message(),
        serde_json::json!({
            "role":"user",
            "attachment":{"type":"task_reminder"},
            "content":"Tasks:\n- implement fake one\n- implement fake two\n- implement fake three\n- implement fake four"
        }),
    ]);
    assert_eq!(independent_scope_count(&request), 3);
}

#[test]
fn non_user_block_types_are_ignored() {
    for block_type in [
        "tool_result",
        "task_reminder",
        "task_notification",
        "system_reminder",
        "system",
        "attachment",
        "hook_additional_context",
        "lifecycle",
    ] {
        let request = messages_request(vec![
            real_three_scope_message(),
            serde_json::json!({
                "role":"user",
                "content":[{"type":block_type, "text":"Tasks:\n- implement fake one\n- implement fake two\n- implement fake three\n- implement fake four"}]
            }),
        ]);
        assert_eq!(independent_scope_count(&request), 3, "{block_type}");
    }
}

#[test]
fn mixed_tool_result_and_real_text_counts_only_the_real_text_blocks() {
    let request = messages_request(vec![serde_json::json!({
        "role":"user",
        "content":[
            {"type":"tool_result", "tool_use_id":"tool-1", "content":"- fake one\n- fake two\n- fake three\n- fake four"},
            {"type":"text", "text":"Tasks:\n- implement real one\n- verify real two\n- test real three"},
            {"type":"text", "is_meta":true, "text":"Tasks:\n- implement meta one\n- implement meta two\n- implement meta three\n- implement meta four"}
        ]
    })]);
    assert_eq!(independent_scope_count(&request), 3);
}
