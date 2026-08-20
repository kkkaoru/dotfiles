use serde_json::Value;

use super::EventTranslateState;
use crate::anthropic::token_efficiency::{
    UnusableCircuitDecision, circuit_is_open, decide_tool_emit, tool_arguments_are_unusable,
};

pub(super) fn should_emit_tool_start(state: &EventTranslateState) -> bool {
    !circuit_is_open(state.consecutive_unusable_tools)
}

pub(super) fn should_forward_finished_tool(
    state: &mut EventTranslateState,
    name: &str,
    arguments: &Value,
) -> bool {
    match decide_tool_emit(
        state.consecutive_unusable_tools,
        0,
        tool_arguments_are_unusable(name, arguments),
    ) {
        UnusableCircuitDecision::Emit { consecutive } => {
            state.consecutive_unusable_tools = consecutive;
            true
        }
        UnusableCircuitDecision::Skip { consecutive, .. } => {
            state.consecutive_unusable_tools = consecutive;
            tracing::warn!(tool = %name, "dropping unusable Pi tool_use after consecutive invalid arguments");
            false
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "events_circuit_tests.rs"]
mod tests;
