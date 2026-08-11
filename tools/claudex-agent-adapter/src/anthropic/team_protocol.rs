use std::borrow::Cow;

use serde_json::Value;

use super::{MessagesRequest, agent_effort::is_agent_tool};

const GUIDANCE: &str = "Determine SubAgent lifecycle from each Agent or Task tool result, because a named SubAgent may be either a persistent mailbox teammate or a regular background agent. A result containing teammate_spawned or saying that the agent receives instructions via mailbox identifies a teammate: never pass that named teammate's name or name@session agent ID to TaskOutput or TaskList; use SendMessage with the teammate name only when further communication is necessary, otherwise end the turn and wait for Claude Code's automatic teammate message. A result saying Async agent launched identifies a regular background agent: wait for Claude Code's automatic completion notification and follow the recipient ID stated by that result if communication is necessary. Never restart a completed agent merely to collect output. Use TaskOutput only when a tool result explicitly returns a task_id for TaskOutput, never with a display name or agent_id. TaskStop/Stop Task is best-effort and idempotent: invoke it only with the exact active task_id returned by the current Agent/Task result; never guess, reuse, stop a display name, mailbox agent_id, previous-session orphan ID from a `No completion record` notification, completion notification, or already-consumed task. If Claude Code reports `No task found` for a stop request, treat the task as already completed or stopped, do not retry, and continue without cascading TaskStop onto unrelated in-flight workers. Keep one in-flight worker per scope/description key; do not fan the same key across multiple models while a peer is still running.";

const RESULT_CLARIFICATION: &str = "Claudex protocol: this is a named mailbox teammate, not a TaskOutput or TaskList task. Do not pass its name or agent_id to TaskOutput. Use SendMessage with the teammate name when needed, then end the turn and wait for automatic teammate messages. Do not call TaskStop/Stop Task for this mailbox result; if a stale stop reports `No task found`, treat it as an idempotent already-stopped outcome and continue.";

pub(super) fn guidance(request: &MessagesRequest) -> Option<&'static str> {
    let named_agent = request.tools.iter().any(|tool| {
        tool.get("name")
            .and_then(Value::as_str)
            .is_some_and(is_agent_tool)
            && tool.pointer("/input_schema/properties/name").is_some()
    });
    let send_message = request
        .tools
        .iter()
        .any(|tool| tool.get("name").and_then(Value::as_str) == Some("SendMessage"));
    let explicit_team = request.tools.iter().any(|tool| {
        let name = tool.get("name").and_then(Value::as_str).unwrap_or_default();
        matches!(
            name,
            "TeamCreate"
                | "TeamDelete"
                | "TeamList"
                | "TeamRename"
                | "TeamUpdate"
                | "TeamSendMessage"
        ) || name.starts_with("team_")
    }) || request.messages.iter().any(|message| {
        let text = message.to_string();
        text.contains("USE_NAMED_TEAM_MAILBOX") || text.contains("<teammate-message")
    });
    (named_agent && send_message && explicit_team).then_some(GUIDANCE)
}

pub(super) fn clarify_result(text: &str) -> Cow<'_, str> {
    if !is_teammate_result(text) {
        return Cow::Borrowed(text);
    }
    if text.contains(RESULT_CLARIFICATION) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(format!("{text}\n\n{RESULT_CLARIFICATION}"))
}

fn is_teammate_result(text: &str) -> bool {
    text.contains("teammate_spawned")
        || text.contains("receive instructions via mailbox")
        || (text.contains("agent_id:") && text.contains("name:") && text.contains("mailbox"))
}

#[cfg(test)]
include!("team_protocol_tests.rs");

