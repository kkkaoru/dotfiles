//! Display-only Anthropic `server_tool_use` cards for SubAgent provider tools.
//!
//! ACP/Cursor/Grok already executed Bash/Read. Emitting Anthropic `tool_use`
//! would re-run them in Claude Code. `server_tool_use` is assistant-side and
//! display-only. High-effort SubAgents otherwise stay on collapsed
//! "Wandering… still thinking with high effort" while only ▶ thinking chrome
//! updates — custom-advisor escapes that because Subscription forwards real
//! `tool_use` cards.

use anyhow::Result;
use serde_json::{Value, json};
use uuid::Uuid;

use super::SegmentBuilder;
use crate::anthropic::stream::protocol::{StreamSender, send_stream_frame};

#[derive(Clone, Copy)]
pub(super) enum ServerKind {
    Bash,
    Editor,
}

impl ServerKind {
    fn use_name(self) -> &'static str {
        match self {
            Self::Bash => "bash_code_execution",
            Self::Editor => "text_editor_code_execution",
        }
    }

    fn result_name(self) -> &'static str {
        match self {
            Self::Bash => "bash_code_execution_tool_result",
            Self::Editor => "text_editor_code_execution_tool_result",
        }
    }

    fn result_content_type(self) -> &'static str {
        match self {
            Self::Bash => "bash_code_execution_result",
            Self::Editor => "text_editor_code_execution_result",
        }
    }
}

impl SegmentBuilder {
    /// Paint a display-only server tool card for SubAgent provider work.
    /// Returns true when ▶ thinking chrome should be skipped for this call.
    pub(in crate::anthropic::stream) async fn emit_subagent_server_tool(
        &mut self,
        call_id: &str,
        tool_name: &str,
        title: &str,
        arguments: Option<&Value>,
        stream: Option<&StreamSender>,
    ) -> Result<bool> {
        if !self.is_subagent {
            return Ok(false);
        }
        let Some(kind) = map_server_kind(tool_name) else {
            return Ok(false);
        };
        if self
            .server_tools
            .iter()
            .any(|(seen, _, _)| seen == call_id)
        {
            return Ok(true);
        }
        self.note_provider_turn_activity();
        self.close_text_block(stream).await?;
        self.thinking.close(&mut self.blocks, stream).await?;
        let srv_id = format!("srvtoolu_{}", Uuid::new_v4().simple());
        let input = server_tool_input(kind, arguments, title);
        emit_server_tool_use(&mut self.blocks, stream, &srv_id, kind.use_name(), &input).await?;
        self.server_tools
            .push((call_id.to_owned(), srv_id, kind));
        Ok(true)
    }

    pub(in crate::anthropic::stream) async fn complete_subagent_server_tool(
        &mut self,
        call_id: &str,
        ok: bool,
        stream: Option<&StreamSender>,
    ) -> Result<bool> {
        let Some(index) = self
            .server_tools
            .iter()
            .position(|(seen, _, _)| seen == call_id)
        else {
            return Ok(false);
        };
        let (_, srv_id, kind) = self.server_tools.remove(index);
        self.note_provider_turn_activity();
        self.close_text_block(stream).await?;
        self.thinking.close(&mut self.blocks, stream).await?;
        emit_server_tool_result(&mut self.blocks, stream, &srv_id, kind, ok).await?;
        Ok(true)
    }
}

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

fn first_arg<'a>(args: Option<&'a Value>, keys: &[&str]) -> Option<&'a str> {
    let map = args?.as_object()?;
    keys.iter()
        .find_map(|key| map.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn server_tool_input(kind: ServerKind, arguments: Option<&Value>, title: &str) -> Value {
    match kind {
        ServerKind::Bash => {
            let command = first_arg(arguments, &["command", "cmd", "script"])
                .or_else(|| meaningful_title(title))
                .unwrap_or("");
            json!({"command": truncate(command, 240)})
        }
        ServerKind::Editor => {
            let path = first_arg(
                arguments,
                &["path", "file_path", "target_file", "file", "pattern", "query"],
            )
            .or_else(|| meaningful_title(title))
            .unwrap_or("");
            json!({
                "command": "view",
                "path": truncate(path, 240)
            })
        }
    }
}

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

fn truncate(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

async fn emit_server_tool_use(
    blocks: &mut Vec<Value>,
    stream: Option<&StreamSender>,
    id: &str,
    name: &str,
    input: &Value,
) -> Result<()> {
    let index = blocks.len();
    blocks.push(json!({
        "type": "server_tool_use",
        "id": id,
        "name": name,
        "input": input
    }));
    send_stream_frame(stream, "content_block_start", || {
        json!({
            "type": "content_block_start",
            "index": index,
            "content_block": {
                "type": "server_tool_use",
                "id": id,
                "name": name,
                "input": {}
            }
        })
    })
    .await?;
    send_stream_frame(stream, "content_block_delta", || {
        json!({
            "type": "content_block_delta",
            "index": index,
            "delta": {
                "type": "input_json_delta",
                "partial_json": input.to_string()
            }
        })
    })
    .await?;
    send_stream_frame(
        stream,
        "content_block_stop",
        || json!({"type": "content_block_stop", "index": index}),
    )
    .await
}

async fn emit_server_tool_result(
    blocks: &mut Vec<Value>,
    stream: Option<&StreamSender>,
    tool_use_id: &str,
    kind: ServerKind,
    ok: bool,
) -> Result<()> {
    let index = blocks.len();
    let content = match kind {
        ServerKind::Bash => json!({
            "type": kind.result_content_type(),
            "stdout": "",
            "stderr": if ok { "" } else { "failed" },
            "return_code": if ok { 0 } else { 1 }
        }),
        ServerKind::Editor => json!({
            "type": kind.result_content_type(),
            "file_type": "text",
            "content": ""
        }),
    };
    let block = json!({
        "type": kind.result_name(),
        "tool_use_id": tool_use_id,
        "content": content
    });
    blocks.push(block.clone());
    send_stream_frame(stream, "content_block_start", || {
        json!({
            "type": "content_block_start",
            "index": index,
            "content_block": block
        })
    })
    .await?;
    send_stream_frame(
        stream,
        "content_block_stop",
        || json!({"type": "content_block_stop", "index": index}),
    )
    .await
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
