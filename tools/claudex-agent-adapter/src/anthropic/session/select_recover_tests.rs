use serde_json::{Value, json};

use super::{
    any_incomplete_tool_use, incomplete_tool_use_count, incomplete_tool_use_ids,
    maybe_sanitize_recovered_request, sanitize_session_loss_replay, tool_use_input_is_complete,
};
use crate::anthropic::MessagesRequest;

#[test]
fn empty_transcript_has_no_incomplete_tool_use() {
    assert!(!any_incomplete_tool_use(&[]));
    assert_eq!(incomplete_tool_use_count(&[]), 0);
    assert!(incomplete_tool_use_ids(&[]).is_empty());
}

#[test]
fn empty_object_is_incomplete_tool_use_input() {
    assert!(!tool_use_input_is_complete(&json!({})));
}

#[test]
fn bash_command_object_is_complete_tool_use_input() {
    assert!(tool_use_input_is_complete(&json!({"command":"ls"})));
}

#[test]
fn truncated_json_string_is_incomplete_tool_use_input() {
    assert!(!tool_use_input_is_complete(&json!("{\"command\":\"cat")));
}

#[test]
fn complete_json_string_is_complete_tool_use_input() {
    assert!(tool_use_input_is_complete(&json!("{\"command\":\"ls\"}")));
}

#[test]
fn empty_object_json_string_is_incomplete_tool_use_input() {
    assert!(!tool_use_input_is_complete(&json!("{}")));
}

#[test]
fn non_object_json_string_is_incomplete_tool_use_input() {
    assert!(!tool_use_input_is_complete(&json!("[]")));
    assert!(!tool_use_input_is_complete(&json!("1")));
}

#[test]
fn null_array_and_number_inputs_are_incomplete() {
    assert!(!tool_use_input_is_complete(&Value::Null));
    assert!(!tool_use_input_is_complete(&json!([])));
    assert!(!tool_use_input_is_complete(&json!(1)));
}

#[test]
fn missing_input_is_treated_as_incomplete() {
    assert!(!tool_use_input_is_complete(&Value::Null));
}

#[test]
fn incomplete_ids_skip_complete_bash_and_nameless_blocks() {
    let ids = incomplete_tool_use_ids(&[
        json!({"role":"assistant","content":[
            {"type":"tool_use","id":"toolu-ok","name":"Bash","input":{"command":"ls"}},
            {"type":"tool_use","id":"toolu-empty","name":"Bash","input":{}},
            {"type":"tool_use","name":"Read","input":{}},
            {"type":"text","text":"ignore"}
        ]}),
        json!({"role":"user","content":"plain"}),
    ]);
    assert_eq!(ids.len(), 1);
    assert!(ids.contains("toolu-empty"));
}

#[test]
fn incomplete_count_includes_nameless_empty_tool_use() {
    assert_eq!(
        incomplete_tool_use_count(&[json!({"role":"assistant","content":[
            {"type":"tool_use","name":"Bash","input":{}},
            {"type":"tool_use","id":"toolu-ok","name":"Read","input":{"path":"CLAUDE.md"}}
        ]})]),
        1
    );
    assert!(!any_incomplete_tool_use(&[
        json!({"role":"assistant","content":[
            {"type":"tool_use","id":"toolu-ok","name":"Read","input":{"path":"CLAUDE.md"}}
        ]})
    ]));
}

#[test]
fn sanitize_keeps_complete_json_and_fails_empty_or_truncated_tool_use() {
    let sanitized = sanitize_session_loss_replay(&[
        json!({"role":"assistant","content":[
            {"type":"text","text":"working"},
            {"type":"tool_use","id":"toolu-ok","name":"Bash","input":{"command":"ls"}},
            {"type":"tool_use","id":"toolu-empty","name":"Bash","input":{}},
            {"type":"tool_use","id":"toolu-trunc","name":"Read","input":"{\"path\":\"CLAUDE"}
        ]}),
        json!({"role":"user","content":[
            {"type":"tool_result","tool_use_id":"toolu-ok","content":"ok"},
            {"type":"tool_result","tool_use_id":"toolu-empty","content":"done"},
            {"type":"tool_result","tool_use_id":"toolu-trunc","content":"nope"}
        ]}),
    ]);
    assert_eq!(
        sanitized,
        vec![
            json!({"role":"assistant","content":[
                {"type":"text","text":"working"},
                {"type":"tool_use","id":"toolu-ok","name":"Bash","input":{"command":"ls"}},
                {"type":"text","text":"Adapter session was lost before tool_use `toolu-empty` (Bash) received complete JSON arguments. The incomplete call was not replayed."},
                {"type":"text","text":"Adapter session was lost before tool_use `toolu-trunc` (Read) received complete JSON arguments. The incomplete call was not replayed."}
            ]}),
            json!({"role":"user","content":[
                {"type":"tool_result","tool_use_id":"toolu-ok","content":"ok"},
                {"type":"text","text":"Adapter session was lost before tool_use `toolu-empty` received complete JSON arguments. The in-flight tool was failed instead of replaying empty or truncated input."},
                {"type":"text","text":"Adapter session was lost before tool_use `toolu-trunc` received complete JSON arguments. The in-flight tool was failed instead of replaying empty or truncated input."}
            ]}),
        ]
    );
}

#[test]
fn sanitize_leaves_plain_messages_unchanged() {
    assert_eq!(
        sanitize_session_loss_replay(&[
            json!({"role":"user","content":"hello"}),
            json!({"role":"assistant","content":"ok"}),
            json!({"role":"user"}),
        ]),
        vec![
            json!({"role":"user","content":"hello"}),
            json!({"role":"assistant","content":"ok"}),
            json!({"role":"user"}),
        ]
    );
}

#[test]
fn sanitize_fails_nameless_empty_tool_use_without_touching_other_results() {
    let sanitized = sanitize_session_loss_replay(&[json!({"role":"assistant","content":[
        {"type":"tool_use","name":"Bash","input":{}},
        {"type":"tool_use","id":"toolu-ok","name":"Read","input":{"path":"README.md"}}
    ]})]);
    assert_eq!(
        sanitized,
        vec![json!({"role":"assistant","content":[
            {"type":"text","text":"Adapter session was lost before tool_use `unknown` (Bash) received complete JSON arguments. The incomplete call was not replayed."},
            {"type":"tool_use","id":"toolu-ok","name":"Read","input":{"path":"README.md"}}
        ]})]
    );
}

#[test]
fn sanitize_keeps_tool_result_without_an_id() {
    assert_eq!(
        sanitize_session_loss_replay(&[
            json!({"role":"user","content":[{"type":"tool_result","content":"done"}]})
        ]),
        vec![json!({"role":"user","content":[{"type":"tool_result","content":"done"}]})]
    );
}

#[test]
fn sanitize_uses_placeholder_name_when_tool_use_omits_name() {
    assert_eq!(
        sanitize_session_loss_replay(&[json!({"role":"assistant","content":[
            {"type":"tool_use","id":"toolu-x","input":{}}
        ]})]),
        vec![json!({"role":"assistant","content":[
            {"type":"text","text":"Adapter session was lost before tool_use `toolu-x` (tool) received complete JSON arguments. The incomplete call was not replayed."}
        ]})]
    );
}

#[test]
fn recovered_request_is_left_alone_when_tool_use_json_is_complete() {
    let request = MessagesRequest {
        model: "main".to_owned(),
        system: Value::Null,
        messages: vec![json!({"role":"assistant","content":[
            {"type":"tool_use","id":"toolu-ok","name":"Bash","input":{"command":"ls"}}
        ]})],
        tools: Vec::new(),
        stream: false,
        output_config: Value::Null,
        metadata: Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };
    assert!(maybe_sanitize_recovered_request(true, &request).is_none());
    assert!(maybe_sanitize_recovered_request(false, &request).is_none());
}

#[test]
fn recovered_request_replaces_empty_bash_tool_use() {
    let request = MessagesRequest {
        model: "main".to_owned(),
        system: Value::Null,
        messages: vec![json!({"role":"assistant","content":[
            {"type":"tool_use","id":"toolu-empty","name":"Bash","input":{}}
        ]})],
        tools: Vec::new(),
        stream: false,
        output_config: Value::Null,
        metadata: Value::Null,
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    };
    assert!(maybe_sanitize_recovered_request(false, &request).is_none());
    let sanitized = maybe_sanitize_recovered_request(true, &request).expect("sanitize empty bash");
    assert_eq!(
        sanitized.messages,
        vec![json!({"role":"assistant","content":[
            {"type":"text","text":"Adapter session was lost before tool_use `toolu-empty` (Bash) received complete JSON arguments. The incomplete call was not replayed."}
        ]})]
    );
}
