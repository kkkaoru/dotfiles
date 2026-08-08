use serde_json::{Value, json};

use super::super::MessagesRequest;

pub(super) const REUSE_GUIDANCE_MARKER: &str = "<claudex-subagent-reuse>";

pub(in crate::anthropic) fn value_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| value_text(Some(value)))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Object(object)) => object
            .get("text")
            .map(|text| value_text(Some(text)))
            .unwrap_or_else(|| {
                object
                    .values()
                    .map(|value| value_text(Some(value)))
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
        _ => String::new(),
    }
}

pub(super) fn has_send_message_tool(tools: &[Value]) -> bool {
    tools.iter().any(|tool| {
        tool.get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| name == "SendMessage")
    })
}

/// A normal Agent/Task session must not use the mailbox transport for worker
/// results. Claude exposes SendMessage only for an explicit Agent Teams
/// session; treating its mere presence as an invitation causes raw
/// `<agent-message>` attachments to be queued as user input.
pub(in crate::anthropic) fn agent_teams_enabled(request: &MessagesRequest) -> bool {
    let team_tool = request.tools.iter().any(|tool| {
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
    });
    let explicit_team_text = request.messages.iter().any(|message| {
        let text = value_text(Some(message));
        text.contains("USE_NAMED_TEAM_MAILBOX") || text.contains("<teammate-message")
    });
    team_tool || explicit_team_text
}

pub(super) fn system_contains_marker(system: &Value) -> bool {
    value_text(Some(system)).contains(REUSE_GUIDANCE_MARKER)
}

pub(super) fn append_reuse_guidance(system: &mut Value, recipients: &[String], teams: bool) {
    let recipients = recipients
        .iter()
        .take(32)
        .map(|recipient| format!("`{recipient}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let guidance = if teams {
        format!(
            "{REUSE_GUIDANCE_MARKER}\nThis resumed Agent Teams session already has compatible teammates and their recorded scopes: {recipients}. Choose the teammate whose recorded scope best matches the current task, then use TeamSendMessage with that exact recipient. Do not assign unrelated work to a teammate merely because it is available. Do not launch a replacement for the same scope. Create a new Agent/Task only for genuinely independent work or when the existing teammate is unavailable.\n</claudex-subagent-reuse>"
        )
    } else {
        format!(
            "{REUSE_GUIDANCE_MARKER}\nThis session already has compatible SubAgents and their recorded scopes: {recipients}. For same-scope follow-up, call Agent or Task with resume set to that exact agentId and the new prompt. Do not launch a replacement for the same scope. Create a new Agent/Task only for genuinely independent work or when the existing worker failed, was cancelled, stopped, or is unavailable. Retrieve prior results with TaskOutput using the exact task_id from the launch result. SendMessage is reserved for an explicitly active Agent Teams session.\n</claudex-subagent-reuse>"
        )
    };
    match system {
        Value::String(text) => {
            text.push_str("\n\n");
            text.push_str(&guidance);
        }
        Value::Array(blocks) => blocks.push(json!({"type":"text","text":guidance})),
        _ => *system = Value::String(guidance),
    }
}
