//! Display-only Anthropic `server_tool_use` helpers.
//!
//! Claude Code 2.1 hides `text_delta` until end_turn. Closing thinking to paint
//! cards collapses the SubAgent viewer to repeating "Thought for Xs", including
//! Command Code Muse Spark. All ACP SubAgents keep one native thinking block
//! and ▶ markers instead. `tool_use` would be re-executed.

use anyhow::Result;
use serde_json::{Value, json};

use super::SegmentBuilder;
use crate::anthropic::stream::protocol::StreamSender;

impl SegmentBuilder {
    pub(in crate::anthropic::stream) async fn emit_subagent_server_tool(
        &mut self,
        call_id: &str,
        tool_name: &str,
        arguments: Option<&Value>,
        stream: Option<&StreamSender>,
    ) -> Result<bool> {
        let _ = (call_id, tool_name, arguments, stream);
        Ok(false)
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
