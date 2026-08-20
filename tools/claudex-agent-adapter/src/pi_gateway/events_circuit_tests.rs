use serde_json::json;

use super::super::EventTranslateState;
use super::{should_emit_tool_start, should_forward_finished_tool};

#[test]
fn forwards_usable_bash_and_resets_the_circuit() {
    let mut state = EventTranslateState::default();
    assert!(should_forward_finished_tool(
        &mut state,
        "Bash",
        &json!({"command": "ls -la"})
    ));
    assert!(should_emit_tool_start(&state));
}

#[test]
fn drops_the_first_unusable_agent_without_opening_the_circuit() {
    let mut state = EventTranslateState::default();
    assert!(!should_forward_finished_tool(
        &mut state,
        "Agent",
        &json!({"run_in_background": true})
    ));
    assert!(should_emit_tool_start(&state));
}

#[test]
fn third_unusable_agent_stops_further_tool_use_emits() {
    let mut state = EventTranslateState::default();
    assert!(!should_forward_finished_tool(
        &mut state,
        "Agent",
        &json!({"run_in_background": true})
    ));
    assert!(!should_forward_finished_tool(
        &mut state,
        "Agent",
        &json!({"_toolName": "Task"})
    ));
    assert!(!should_forward_finished_tool(
        &mut state,
        "Agent",
        &json!({})
    ));
    assert!(!should_emit_tool_start(&state));
    assert!(!should_forward_finished_tool(
        &mut state,
        "Agent",
        &json!({"prompt": "too late"})
    ));
}
