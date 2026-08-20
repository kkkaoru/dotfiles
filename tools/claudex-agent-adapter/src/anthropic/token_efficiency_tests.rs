use anyhow::anyhow;
use serde_json::json;

use super::{
    CONSECUTIVE_UNUSABLE_TOOL_LIMIT, UnusableCircuitDecision, consecutive_unusable_from_messages,
    decide_tool_emit, is_input_validation_error, is_provider_cooldown_error,
    should_retry_provider_failure, tool_arguments_are_unusable, truncated_tool_result_text,
};

#[test]
fn named_circuit_limit_is_three() {
    assert_eq!(CONSECUTIVE_UNUSABLE_TOOL_LIMIT, 3);
}

#[test]
fn usable_args_reset_the_circuit() {
    assert_eq!(
        decide_tool_emit(2, 0, false),
        UnusableCircuitDecision::Emit { consecutive: 0 }
    );
}

#[test]
fn first_unusable_args_skip_without_ending_the_turn() {
    assert_eq!(
        decide_tool_emit(0, 0, true),
        UnusableCircuitDecision::Skip {
            consecutive: 1,
            end_turn: false
        }
    );
}

#[test]
fn second_unusable_args_skip_without_ending_the_turn() {
    assert_eq!(
        decide_tool_emit(1, 0, true),
        UnusableCircuitDecision::Skip {
            consecutive: 2,
            end_turn: false
        }
    );
}

#[test]
fn third_consecutive_unusable_args_end_the_turn() {
    assert_eq!(
        decide_tool_emit(2, 0, true),
        UnusableCircuitDecision::Skip {
            consecutive: 3,
            end_turn: true
        }
    );
}

#[test]
fn already_open_circuit_emits_no_more_tool_use() {
    assert_eq!(
        decide_tool_emit(3, 0, false),
        UnusableCircuitDecision::Skip {
            consecutive: 3,
            end_turn: true
        }
    );
}

#[test]
fn three_input_validation_errors_in_messages_end_the_turn() {
    assert_eq!(
        decide_tool_emit(0, 3, false),
        UnusableCircuitDecision::Skip {
            consecutive: 3,
            end_turn: true
        }
    );
}

#[test]
fn agent_without_prompt_is_unusable() {
    assert!(tool_arguments_are_unusable(
        "Agent",
        &json!({"run_in_background": true})
    ));
}

#[test]
fn agent_with_prompt_is_usable() {
    assert!(!tool_arguments_are_unusable(
        "Agent",
        &json!({"prompt": "Handle the bounded review."})
    ));
}

#[test]
fn bash_without_command_is_unusable() {
    assert!(tool_arguments_are_unusable("Bash", &json!({})));
}

#[test]
fn bash_with_command_is_usable() {
    assert!(!tool_arguments_are_unusable(
        "Bash",
        &json!({"command": "ls -la"})
    ));
}

#[test]
fn send_message_without_body_is_unusable() {
    assert!(tool_arguments_are_unusable(
        "SendMessage",
        &json!({"to": "worker-a"})
    ));
}

#[test]
fn send_message_with_to_and_message_is_usable() {
    assert!(!tool_arguments_are_unusable(
        "SendMessage",
        &json!({"to": "worker-a", "message": "continue"})
    ));
}

#[test]
fn empty_read_object_is_unusable() {
    assert!(tool_arguments_are_unusable("Read", &json!({})));
}

#[test]
fn read_with_path_is_usable() {
    assert!(!tool_arguments_are_unusable(
        "Read",
        &json!({"path": "CLAUDE.md"})
    ));
}

#[test]
fn detects_input_validation_error_text() {
    assert!(is_input_validation_error(
        "InputValidationError: prompt: Required"
    ));
    assert!(!is_input_validation_error("tool failed: file not found"));
}

#[test]
fn detects_provider_cooldown_text() {
    assert!(is_provider_cooldown_error(
        "API Error: 502 grok-4.6 is cooling down after a no-event prompt timeout"
    ));
    assert!(!is_provider_cooldown_error("transient 502 from the proxy"));
}

#[test]
fn does_not_retry_cooldown_502_or_input_validation_error() {
    assert!(!should_retry_provider_failure(&anyhow!(
        "API Error: 502 grok-4.6 is cooling down after a no-event prompt timeout"
    )));
    assert!(!should_retry_provider_failure(&anyhow!(
        "InputValidationError: prompt: Required"
    )));
    assert!(should_retry_provider_failure(&anyhow!(
        "app-server event stream closed"
    )));
}

#[test]
fn consecutive_input_validation_errors_count_from_trailing_user_results() {
    let messages = json!([
        {"role":"assistant","content":[{"type":"tool_use","id":"one","name":"Agent","input":{}}]},
        {"role":"user","content":[
            {"type":"tool_result","tool_use_id":"one","is_error":true,"content":"InputValidationError: prompt: Required"},
            {"type":"tool_result","tool_use_id":"two","is_error":true,"content":"InputValidationError: prompt: Required"},
            {"type":"tool_result","tool_use_id":"three","is_error":true,"content":"InputValidationError: prompt: Required"}
        ]}
    ]);
    assert_eq!(
        consecutive_unusable_from_messages(messages.as_array().expect("messages array")),
        3
    );
}

#[test]
fn successful_tool_result_breaks_the_input_validation_streak() {
    let messages = json!([
        {"role":"user","content":[
            {"type":"tool_result","tool_use_id":"ok","is_error":false,"content":"done"},
            {"type":"tool_result","tool_use_id":"bad","is_error":true,"content":"InputValidationError: prompt: Required"}
        ]}
    ]);
    assert_eq!(
        consecutive_unusable_from_messages(messages.as_array().expect("messages array")),
        1
    );
}

#[test]
fn truncates_huge_tool_results_to_the_named_byte_budget() {
    let truncated = truncated_tool_result_text(&"x".repeat(40000));
    assert_eq!(truncated.len(), 32768);
    assert_eq!(&truncated[..4], "xxxx");
    assert_eq!(
        &truncated[32724..],
        "\n[tool_result truncated to fit token budget]"
    );
}

#[test]
fn keeps_short_tool_results_unchanged() {
    assert_eq!(truncated_tool_result_text("done"), "done");
}
