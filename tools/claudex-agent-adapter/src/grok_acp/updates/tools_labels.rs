use agent_client_protocol as acp;
use serde_json::Value;

pub(super) fn tool_content_text(content: &[acp::ToolCallContent]) -> String {
    content
        .iter()
        .filter_map(tool_content_part)
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_content_part(item: &acp::ToolCallContent) -> Option<String> {
    match item {
        acp::ToolCallContent::Content(block) => match &block.content {
            acp::ContentBlock::Text(text) if !text.text.is_empty() => Some(text.text.clone()),
            _ => None,
        },
        acp::ToolCallContent::Diff(diff) => {
            let path = diff.path.display();
            let old = diff.old_text.as_deref().unwrap_or("");
            Some(format!(
                "diff {path}:\n--- old ---\n{old}\n--- new ---\n{}",
                diff.new_text
            ))
        }
        acp::ToolCallContent::Terminal(term) => {
            Some(format!("terminal {term_id}", term_id = term.terminal_id))
        }
        _ => None,
    }
}

pub(super) fn tool_display_name(call: &acp::ToolCall) -> String {
    // Prefer explicit tool names from Cursor MCP / provider meta over generic
    // titles like "MCP" so launch bridging can map Agent/Task correctly.
    if let Some(name) = call.raw_input.as_ref().and_then(|input| {
        ["_toolName", "toolName", "name", "tool"]
            .into_iter()
            .find_map(|key| input.get(key).and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }) {
        return name.to_owned();
    }
    if let Some(from_kind) = tool_kind_name(call.kind) {
        return from_kind.into();
    }
    let title = call.title.trim();
    let stripped = title
        .strip_prefix("Using ")
        .unwrap_or(title)
        .trim_end_matches('…')
        .trim_end_matches("...")
        .trim();
    if let Some((head, _)) = stripped.split_once(':') {
        let head = head.trim();
        if !head.is_empty() && !head.contains(' ') {
            return head.to_owned();
        }
    }
    if stripped.is_empty() {
        "Tool".into()
    } else {
        stripped.to_owned()
    }
}

pub(super) fn tool_kind_name(kind: acp::ToolKind) -> Option<&'static str> {
    match kind {
        acp::ToolKind::Read => Some("Read"),
        acp::ToolKind::Edit => Some("Edit"),
        acp::ToolKind::Execute => Some("Bash"),
        acp::ToolKind::Search => Some("Search"),
        acp::ToolKind::Fetch => Some("WebFetch"),
        acp::ToolKind::Delete => Some("Delete"),
        acp::ToolKind::Move => Some("Move"),
        acp::ToolKind::Think => Some("Think"),
        acp::ToolKind::SwitchMode => Some("SwitchMode"),
        _ => None,
    }
}

pub(super) fn tool_kind_label(kind: acp::ToolKind) -> &'static str {
    tool_kind_name(kind).unwrap_or("other")
}

pub(super) fn tool_status_label(status: acp::ToolCallStatus) -> &'static str {
    match status {
        acp::ToolCallStatus::Completed => "completed",
        acp::ToolCallStatus::Failed => "failed",
        acp::ToolCallStatus::InProgress => "in_progress",
        acp::ToolCallStatus::Pending => "pending",
        _ => "updated",
    }
}
