//! Display-only Anthropic `server_tool_use` cards for ACP SubAgents.
//!
//! Claude Code 2.1 hides `text_delta` until end_turn. Closing thinking to paint
//! cards collapses the SubAgent viewer to "Thought for Xs" / Spelunking, so ACP
//! SubAgents keep one native thinking block. Command Code still uses
//! `server_tool_use` (same family as Anthropic web_search). `tool_use` would be
//! re-executed. Codex SubAgents already paint via real `tool_use`.

use anyhow::Result;
use serde_json::{Value, json};
use uuid::Uuid;

use super::SegmentBuilder;
use crate::anthropic::stream::protocol::{StreamSender, send_stream_frame};

impl SegmentBuilder {
    pub(in crate::anthropic::stream) async fn emit_subagent_server_tool(
        &mut self,
        call_id: &str,
        tool_name: &str,
        arguments: Option<&Value>,
        stream: Option<&StreamSender>,
    ) -> Result<bool> {
        if !self.is_subagent {
            return Ok(false);
        }
        if tool_name.to_ascii_lowercase().contains("command code") {
            return Ok(false);
        }
        if !self.is_command_code_subagent() {
            // Closing thinking to paint server_tool_use collapses Claude Code 2.1
            // SubAgent chrome to "Thought for Xs" and later thoughts never resync.
            // ACP SubAgents keep one native thinking block and ▶ markers instead.
            return Ok(false);
        }
        let server_name = server_tool_name(tool_name);
        if self.server_tools.iter().any(|(seen, _, _)| seen == call_id) {
            return Ok(suppress_thinking_chrome(tool_name));
        }
        self.close_text_block(stream).await?;
        self.thinking.close(&mut self.blocks, stream).await?;
        let srv_id = format!("srvtoolu_{}", Uuid::new_v4().simple());
        let input = server_tool_input(server_name, arguments, tool_name);
        emit_server_tool_use(&mut self.blocks, stream, &srv_id, server_name, &input).await?;
        self.server_tools
            .push((call_id.to_owned(), srv_id, server_name));
        Ok(suppress_thinking_chrome(tool_name))
    }
}

fn server_tool_name(tool: &str) -> &'static str {
    let lower = tool.to_ascii_lowercase();
    if lower.contains("fetch") {
        "web_fetch"
    } else {
        "web_search"
    }
}

fn suppress_thinking_chrome(tool: &str) -> bool {
    let lower = tool.to_ascii_lowercase();
    lower.contains("fetch") || lower.contains("search") || lower.contains("web")
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

fn meaningful_title(title: &str) -> Option<&str> {
    let trimmed = title.trim();
    let lower = trimmed.to_ascii_lowercase();
    (!trimmed.is_empty()
        && lower != "web_search"
        && lower != "search"
        && lower != "web"
        && lower != "web_fetch"
        && lower != "webfetch")
        .then_some(trimmed)
}

fn server_tool_input(server_name: &str, arguments: Option<&Value>, title: &str) -> Value {
    let args = arguments.and_then(Value::as_object);
    match server_name {
        "web_fetch" => {
            let url = first_arg(args, &["url", "path", "file_path", "target_file"])
                .or_else(|| title_detail(title))
                .unwrap_or("");
            json!({"url": url})
        }
        _ => {
            let query = first_arg(
                args,
                &[
                    "query",
                    "description",
                    "pattern",
                    "command",
                    "cmd",
                    "path",
                    "file_path",
                    "target_file",
                    "url",
                ],
            )
            .or_else(|| title_detail(title))
            .or_else(|| meaningful_title(title))
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
    fn maps_acp_tools_to_display_only_server_cards() {
        assert_eq!(server_tool_name("web_search"), "web_search");
        assert_eq!(server_tool_name("WebSearch"), "web_search");
        assert_eq!(server_tool_name("web_fetch"), "web_fetch");
        assert_eq!(server_tool_name("WebFetch"), "web_fetch");
        assert_eq!(server_tool_name("read_file"), "web_search");
        assert_eq!(server_tool_name("Read"), "web_search");
        assert_eq!(server_tool_name("Bash"), "web_search");
        assert_eq!(server_tool_name("Grep"), "web_search");
    }

    #[test]
    fn web_search_query_falls_back_to_meaningful_title() {
        use super::server_tool_input;
        use serde_json::json;
        assert_eq!(
            server_tool_input("web_search", Some(&json!({})), "函館 天気"),
            json!({"query": "函館 天気"})
        );
        assert_eq!(
            server_tool_input("web_search", Some(&json!({})), "web_search"),
            json!({"query": ""})
        );
        assert_eq!(
            server_tool_input(
                "web_search",
                Some(&json!({"query": "大阪 天気"})),
                "web_search"
            ),
            json!({"query": "大阪 天気"})
        );
        assert_eq!(
            server_tool_input(
                "web_search",
                Some(&json!({"path": "apps/local-postgresql/CLAUDE.md"})),
                "Read File"
            ),
            json!({"query": "apps/local-postgresql/CLAUDE.md"})
        );
        assert_eq!(
            server_tool_input(
                "web_search",
                Some(&json!({"command": "psql -h 127.0.0.1 -p 15432 -c 'select 1'"})),
                "Bash"
            ),
            json!({"query": "psql -h 127.0.0.1 -p 15432 -c 'select 1'"})
        );
    }
}
