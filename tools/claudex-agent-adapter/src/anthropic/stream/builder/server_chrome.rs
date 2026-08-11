//! SubAgent provider-tool display policy.
//!
//! Closing thinking to paint `server_tool_use` made Claude Code 2.1 collapse
//! each block to "Thought for Xs", hiding the live ▶ body. Keep progress on
//! one open thinking block instead. Mapping helpers remain under test for a
//! future card path that does not close thinking.

use anyhow::Result;
use serde_json::Value;

use super::SegmentBuilder;
use crate::anthropic::stream::protocol::StreamSender;

impl SegmentBuilder {
    /// Prefer keep-open ▶ thinking chrome; never close thinking for cards.
    pub(in crate::anthropic::stream) async fn emit_subagent_server_tool(
        &self,
        call_id: &str,
        tool_name: &str,
        title: &str,
        arguments: Option<&Value>,
        stream: Option<&StreamSender>,
    ) -> Result<bool> {
        let _ = (call_id, tool_name, title, arguments, stream);
        Ok(false)
    }

    pub(in crate::anthropic::stream) async fn complete_subagent_server_tool(
        &self,
        call_id: &str,
        ok: bool,
        stream: Option<&StreamSender>,
    ) -> Result<bool> {
        let _ = (call_id, ok, stream);
        Ok(false)
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
enum ServerKind {
    Bash,
    Editor,
}

#[cfg(test)]
fn map_server_kind(tool: &str) -> Option<ServerKind> {
    let lower = tool.to_ascii_lowercase();
    if lower.contains("web_search")
        || lower.contains("websearch")
        || lower.contains("web_fetch")
        || lower.contains("webfetch")
        || lower.contains("web fetch")
    {
        return None;
    }
    if lower.contains("bash")
        || lower.contains("shell")
        || lower.contains("terminal")
        || lower == "cmd"
        || lower.contains("execute")
    {
        return Some(ServerKind::Bash);
    }
    if lower.contains("read")
        || lower.contains("write")
        || lower.contains("edit")
        || lower.contains("grep")
        || lower.contains("glob")
        || lower.contains("file")
    {
        return Some(ServerKind::Editor);
    }
    None
}

#[cfg(test)]
fn first_arg<'a>(args: Option<&'a Value>, keys: &[&str]) -> Option<&'a str> {
    let map = args?.as_object()?;
    keys.iter()
        .find_map(|key| map.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
fn meaningful_title(title: &str) -> Option<&str> {
    let trimmed = title.trim();
    let lower = trimmed.to_ascii_lowercase();
    (!trimmed.is_empty()
        && lower != "bash"
        && lower != "shell"
        && lower != "read"
        && lower != "grep"
        && !lower.starts_with('▶'))
    .then_some(trimmed)
}

#[cfg(test)]
fn truncate(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
fn server_tool_input(kind: ServerKind, arguments: Option<&Value>, title: &str) -> Value {
    match kind {
        ServerKind::Bash => {
            let command = first_arg(arguments, &["command", "cmd", "script"])
                .or_else(|| meaningful_title(title))
                .unwrap_or("");
            serde_json::json!({"command": truncate(command, 240)})
        }
        ServerKind::Editor => {
            let path = first_arg(
                arguments,
                &["path", "file_path", "target_file", "file", "pattern", "query"],
            )
            .or_else(|| meaningful_title(title))
            .unwrap_or("");
            serde_json::json!({
                "command": "view",
                "path": truncate(path, 240)
            })
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{map_server_kind, server_tool_input, ServerKind};
    use serde_json::json;

    #[test]
    fn maps_common_provider_tools() {
        assert!(matches!(map_server_kind("Bash"), Some(ServerKind::Bash)));
        assert!(matches!(map_server_kind("Shell"), Some(ServerKind::Bash)));
        assert!(matches!(map_server_kind("Read"), Some(ServerKind::Editor)));
        assert!(matches!(map_server_kind("Grep"), Some(ServerKind::Editor)));
        assert!(map_server_kind("WebSearch").is_none());
    }

    #[test]
    fn bash_input_uses_command() {
        assert_eq!(
            server_tool_input(
                ServerKind::Bash,
                Some(&json!({"command": "ls apps"})),
                "Bash"
            ),
            json!({"command": "ls apps"})
        );
    }
}
