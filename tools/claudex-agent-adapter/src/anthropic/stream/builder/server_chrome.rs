//! Display-only Anthropic `server_tool_use` cards for Command Code SubAgents.
//!
//! Claude Code 2.1 hides `text_delta` until end_turn and collapses thinking into
//! Spelunking. `tool_use` would be re-executed. `server_tool_use` is the native
//! non-executable web card type (same family as Anthropic web_search).

use anyhow::Result;
use serde_json::{Value, json};
use uuid::Uuid;

use super::SegmentBuilder;
use crate::anthropic::stream::protocol::{StreamSender, send_stream_frame};

impl SegmentBuilder {
    pub(in crate::anthropic::stream) async fn emit_command_code_server_tool(
        &mut self,
        call_id: &str,
        tool_name: &str,
        arguments: Option<&Value>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if !self.is_command_code_subagent() {
            return Ok(());
        }
        let Some(server_name) = server_tool_name(tool_name) else {
            return Ok(());
        };
        if self
            .cc_server_tools
            .iter()
            .any(|(seen, _, _)| seen == call_id)
        {
            return Ok(());
        }
        self.close_text_block(stream).await?;
        self.thinking.close(&mut self.blocks, stream).await?;
        let srv_id = format!("srvtoolu_{}", Uuid::new_v4().simple());
        let input = server_tool_input(server_name, arguments, tool_name);
        emit_server_tool_use(&mut self.blocks, stream, &srv_id, server_name, &input).await?;
        self.cc_server_tools
            .push((call_id.to_owned(), srv_id, server_name));
        Ok(())
    }
}

fn server_tool_name(tool: &str) -> Option<&'static str> {
    let lower = tool.to_ascii_lowercase();
    if lower.contains("fetch") {
        Some("web_fetch")
    } else if lower.contains("search") || lower.contains("web") {
        Some("web_search")
    } else {
        None
    }
}

fn first_arg<'a>(
    args: Option<&'a serde_json::Map<String, Value>>,
    keys: &[&str],
) -> Option<&'a str> {
    let map = args?;
    keys.iter()
        .find_map(|key| map.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn title_detail(title: &str) -> Option<&str> {
    title
        .split_once(':')
        .map(|(_, rest)| rest.trim())
        .filter(|value| !value.is_empty())
}

fn server_tool_input(server_name: &str, arguments: Option<&Value>, title: &str) -> Value {
    let args = arguments.and_then(Value::as_object);
    match server_name {
        "web_fetch" => {
            let url = first_arg(args, &["url", "path"])
                .or_else(|| title_detail(title))
                .unwrap_or("");
            json!({"url": url})
        }
        _ => {
            let query = first_arg(args, &["query", "description", "pattern"])
                .or_else(|| title_detail(title))
                .unwrap_or("");
            json!({"query": query})
        }
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::server_tool_name;

    #[test]
    fn maps_command_code_web_tools_only() {
        assert_eq!(server_tool_name("web_search"), Some("web_search"));
        assert_eq!(server_tool_name("WebSearch"), Some("web_search"));
        assert_eq!(server_tool_name("web_fetch"), Some("web_fetch"));
        assert_eq!(server_tool_name("WebFetch"), Some("web_fetch"));
        assert_eq!(server_tool_name("read_file"), None);
        assert_eq!(server_tool_name("Bash"), None);
    }
}
