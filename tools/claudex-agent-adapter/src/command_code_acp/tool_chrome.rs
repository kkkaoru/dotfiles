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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_tool_kinds() {
        assert_eq!(tool_kind("ReadFile"), acp::ToolKind::Read);
        assert_eq!(tool_kind("Grep"), acp::ToolKind::Read);
        assert_eq!(tool_kind("Glob"), acp::ToolKind::Read);
        assert_eq!(tool_kind("WriteFile"), acp::ToolKind::Edit);
        assert_eq!(tool_kind("EditFile"), acp::ToolKind::Edit);
        assert_eq!(tool_kind("ApplyPatch"), acp::ToolKind::Edit);
        assert_eq!(tool_kind("Bash"), acp::ToolKind::Execute);
        assert_eq!(tool_kind("Shell"), acp::ToolKind::Execute);
        assert_eq!(tool_kind("Exec"), acp::ToolKind::Execute);
        assert_eq!(tool_kind("WebSearch"), acp::ToolKind::Search);
        assert_eq!(tool_kind("SearchWeb"), acp::ToolKind::Search);
        assert_eq!(tool_kind("WebFetch"), acp::ToolKind::Search);
        assert_eq!(tool_kind("OpenWeb"), acp::ToolKind::Search);
        assert_eq!(tool_kind("Notify"), acp::ToolKind::Other);
    }

    #[test]
    fn maps_argument_keys_for_known_tools() {
        assert_eq!(argument_key("Grep"), "pattern");
        assert_eq!(argument_key("WebFetch"), "url");
        assert_eq!(argument_key("WebSearch"), "query");
        assert_eq!(argument_key("SearchDocs"), "query");
        assert_eq!(argument_key("OpenWeb"), "query");
        assert_eq!(argument_key("BrowserWeb"), "query");
        assert_eq!(argument_key("Bash"), "command");
        assert_eq!(argument_key("Shell"), "command");
        assert_eq!(argument_key("Exec"), "command");
        assert_eq!(argument_key("ReadFile"), "path");
        assert_eq!(argument_key("WriteFile"), "path");
        assert_eq!(argument_key("EditFile"), "path");
        assert_eq!(argument_key("Glob"), "path");
        assert_eq!(argument_key("Notify"), "description");
    }

    #[test]
    fn builds_raw_input_objects_from_argument_text() {
        assert_eq!(
            tool_raw_input("Bash", Some(" ls -la ")),
            json!({"command": "ls -la"})
        );
        assert_eq!(tool_raw_input("Read", None), json!({}));
        assert_eq!(tool_raw_input("Read", Some("   ")), json!({}));
        assert_eq!(
            tool_raw_input("Notify", Some("hi")),
            json!({"description": "hi"})
        );
    }
}
