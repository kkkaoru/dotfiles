//! Map native Command Code tool events onto the shared ACP ToolCall chrome.
//!
//! Cursor/Qwen/Grok/Cline paint `\n▶ {name}: {query|path|url|command}\n` from
//! `ToolCall.title` + `raw_input` arguments. Keep the title as the tool name
//! only — `compact_title` drops everything after the first `:`.

use agent_client_protocol as acp;
use serde_json::{Value, json};

pub fn tool_kind(name: &str) -> acp::ToolKind {
    let lower = name.to_ascii_lowercase();
    if lower.contains("read") || lower.contains("grep") || lower.contains("glob") {
        acp::ToolKind::Read
    } else if lower.contains("write") || lower.contains("edit") || lower.contains("patch") {
        acp::ToolKind::Edit
    } else if lower.contains("bash") || lower.contains("shell") || lower.contains("exec") {
        acp::ToolKind::Execute
    } else if lower.contains("search") || lower.contains("web") {
        acp::ToolKind::Search
    } else {
        acp::ToolKind::Other
    }
}

pub fn tool_raw_input(name: &str, description: Option<&str>) -> Value {
    match description.map(str::trim).filter(|text| !text.is_empty()) {
        Some(detail) => json!({ argument_key(name): detail }),
        None => json!({}),
    }
}

fn argument_key(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.contains("grep") {
        "pattern"
    } else if lower.contains("fetch") {
        "url"
    } else if lower.contains("search") || lower.contains("web") {
        "query"
    } else if lower.contains("bash") || lower.contains("shell") || lower.contains("exec") {
        "command"
    } else if lower.contains("read")
        || lower.contains("write")
        || lower.contains("edit")
        || lower.contains("glob")
    {
        "path"
    } else {
        "description"
    }
}
