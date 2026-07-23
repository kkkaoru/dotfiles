//! ACP ToolCall / Plan → Claude Code display helpers (shared Grok + Copilot).

use agent_client_protocol::{self as acp};
use serde_json::{Map, Value, json};

use crate::app_server::events::ThreadEventDispatcher;

use super::{PROVIDER_TOOL_CALL, PROVIDER_TOOL_UPDATE, dispatch_status};

pub(super) fn dispatch_provider_tool_call(
    events: &ThreadEventDispatcher,
    session_id: &str,
    call: acp::ToolCall,
) {
    let call_id = call.tool_call_id.0.to_string();
    let name = tool_display_name(&call);
    let input = build_tool_input(&call);
    events.dispatch(json!({
        "method": PROVIDER_TOOL_CALL,
        "params": {
            "threadId": session_id,
            "callId": call_id,
            "tool": name,
            "title": call.title,
            "kind": tool_kind_label(call.kind),
            "status": tool_status_label(call.status),
            "arguments": input
        }
    }));
}

pub(super) fn dispatch_provider_tool_update(
    events: &ThreadEventDispatcher,
    session_id: &str,
    update: acp::ToolCallUpdate,
) {
    let call_id = update.tool_call_id.0.to_string();
    let fields = update.fields;
    if let (Some(title), Some(status)) = (fields.title.clone(), fields.status) {
        if matches!(
            status,
            acp::ToolCallStatus::Pending | acp::ToolCallStatus::InProgress
        ) && fields.raw_input.is_some()
        {
            let mut call = acp::ToolCall::new(call_id.clone(), title);
            if let Some(kind) = fields.kind {
                call = call.kind(kind);
            }
            call = call.status(status);
            if let Some(raw_input) = fields.raw_input.clone() {
                call = call.raw_input(raw_input);
            }
            if let Some(content) = fields.content.clone() {
                call = call.content(content);
            }
            if let Some(locations) = fields.locations.clone() {
                call = call.locations(locations);
            }
            dispatch_provider_tool_call(events, session_id, call);
        }
    }

    let Some(status) = fields.status else {
        if fields.raw_input.is_some() || fields.content.is_some() || fields.locations.is_some() {
            let mut params = json!({
                "threadId": session_id,
                "callId": call_id,
                "status": "pending"
            });
            if let Some(title) = fields.title {
                params["title"] = json!(title);
            }
            if let Some(raw_input) = fields.raw_input {
                params["arguments"] =
                    enrich_arguments(raw_input, &fields.content, &fields.locations);
            } else if fields.content.is_some() || fields.locations.is_some() {
                params["arguments"] =
                    enrich_arguments(json!({}), &fields.content, &fields.locations);
            }
            events.dispatch(json!({ "method": PROVIDER_TOOL_UPDATE, "params": params }));
        }
        return;
    };
    let status = tool_status_label(status);
    let mut params = json!({
        "threadId": session_id,
        "callId": call_id,
        "status": status
    });
    if let Some(title) = fields.title {
        params["title"] = json!(title);
    }
    if let Some(raw_input) = fields.raw_input {
        params["arguments"] = enrich_arguments(raw_input, &fields.content, &fields.locations);
    }
    if let Some(output) = combine_output(fields.raw_output, fields.content.as_ref()) {
        params["output"] = output;
    }
    events.dispatch(json!({
        "method": PROVIDER_TOOL_UPDATE,
        "params": params
    }));
}

pub(super) fn dispatch_plan(events: &ThreadEventDispatcher, session_id: &str, plan: acp::Plan) {
    if plan.entries.is_empty() {
        return;
    }
    let mut text = String::from("\n\nPlan:\n");
    for entry in plan.entries {
        let mark = match entry.status {
            acp::PlanEntryStatus::Completed => "●",
            acp::PlanEntryStatus::InProgress => "◎",
            acp::PlanEntryStatus::Pending => "○",
            _ => "·",
        };
        text.push_str(mark);
        text.push(' ');
        text.push_str(entry.content.trim());
        text.push('\n');
    }
    dispatch_status(events, session_id, text);
}

fn build_tool_input(call: &acp::ToolCall) -> Value {
    enrich_arguments(
        call.raw_input
            .clone()
            .unwrap_or_else(|| json!({"description": call.title})),
        &Some(call.content.clone()),
        &Some(call.locations.clone()),
    )
}

fn enrich_arguments(
    raw_input: Value,
    content: &Option<Vec<acp::ToolCallContent>>,
    locations: &Option<Vec<acp::ToolCallLocation>>,
) -> Value {
    let mut object = match raw_input {
        Value::Object(map) => map,
        other if !other.is_null() => {
            let mut map = Map::new();
            map.insert("value".into(), other);
            map
        }
        _ => Map::new(),
    };
    if let Some(paths) = locations.as_ref().filter(|items| !items.is_empty()) {
        object.insert(
            "locations".into(),
            Value::Array(paths.iter().map(tool_location).collect()),
        );
    }
    if let Some(content) = content {
        let text = tool_content_text(content);
        if !text.is_empty() {
            object.entry("content".to_owned()).or_insert(json!(text));
        }
    }
    if object.is_empty() {
        json!({})
    } else {
        Value::Object(object)
    }
}

fn tool_location(location: &acp::ToolCallLocation) -> Value {
    let mut entry = json!({"path": location.path.display().to_string()});
    if let Some(line) = location.line {
        entry["line"] = json!(line);
    }
    entry
}

fn combine_output(
    raw_output: Option<Value>,
    content: Option<&Vec<acp::ToolCallContent>>,
) -> Option<Value> {
    let content_text = content
        .map(|items| tool_content_text(items.as_slice()))
        .unwrap_or_default();
    match (raw_output, content_text.as_str()) {
        (Some(Value::String(s)), extra) if !extra.is_empty() && s != extra => {
            Some(json!(format!("{s}\n{extra}")))
        }
        (Some(value), _) => Some(value),
        (None, extra) if !extra.is_empty() => Some(json!(extra)),
        _ => None,
    }
}

fn tool_content_text(content: &[acp::ToolCallContent]) -> String {
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

fn tool_display_name(call: &acp::ToolCall) -> String {
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

fn tool_kind_name(kind: acp::ToolKind) -> Option<&'static str> {
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

fn tool_kind_label(kind: acp::ToolKind) -> &'static str {
    tool_kind_name(kind).unwrap_or("other")
}

fn tool_status_label(status: acp::ToolCallStatus) -> &'static str {
    match status {
        acp::ToolCallStatus::Completed => "completed",
        acp::ToolCallStatus::Failed => "failed",
        acp::ToolCallStatus::InProgress => "in_progress",
        acp::ToolCallStatus::Pending => "pending",
        _ => "updated",
    }
}

#[cfg(test)]
include!("tools_tests.rs");
