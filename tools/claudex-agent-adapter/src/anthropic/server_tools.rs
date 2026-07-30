use std::{convert::Infallible, sync::Arc};

use anyhow::{Context, Result, bail};
use axum::{
    body::{Body, Bytes},
    http::Response,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{
    Bridge, MessagesRequest, Segment, Usage,
    content::{anthropic_response, estimated_tokens},
    request_routing::official_claude_haiku_model,
    stream::{message_start, send_stream_frame, streaming_sse_response},
    subscription::{SubscriptionOptions, run_subscription_model},
    subscription_request::subscription_request_cwd,
};

const WEB_SEARCH_TYPE_PREFIX: &str = "web_search_";
const WEB_SEARCH_TOOL: &str = "web_search";
const WEB_SEARCH_MODEL_PROMPT: &str = r#"Perform exactly one web search with the WebSearch tool.
Return only a valid JSON array. Each array item must contain the keys "title", "url",
"page_age", and "encrypted_content". Use the search results only; do not invent URLs.
If the search tool fails or returns no results, do not return an empty successful array: explain
the failure in plain text instead."#;

pub(super) fn is_web_search_handoff(request: &MessagesRequest) -> bool {
    request.tools.iter().any(|tool| {
        tool.get("name").and_then(Value::as_str) == Some(WEB_SEARCH_TOOL)
            && tool
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with(WEB_SEARCH_TYPE_PREFIX))
    })
}

impl Bridge {
    pub(super) async fn web_search_handoff(
        &self,
        request: &MessagesRequest,
    ) -> Result<Response<Body>> {
        let query = search_query(request)?;
        let tool_use_id = format!("srvtoolu_{}", Uuid::new_v4().simple());
        let prompt = search_prompt(&query, request);
        let options = SubscriptionOptions {
            effort: Some("max".to_owned()),
            tools: vec!["WebSearch".to_owned()],
            bridge_tools: false,
            cwd: subscription_request_cwd(request),
            slots: Arc::clone(&self.subscription_slots),
            timeout: self.subscription_timeout,
            tool_context: None,
        };
        let raw = run_subscription_model(
            &self.subscription_program,
            official_claude_haiku_model(),
            &prompt,
            options,
        )
        .await
        .context("Claude Haiku WebSearch handoff failed")?;
        tracing::debug!(
            raw_len = raw.len(),
            first_char = raw.chars().next().map(|character| character.to_string()),
            has_array = raw.contains('['),
            has_url = raw.contains("http://") || raw.contains("https://"),
            "Claude Haiku WebSearch output shape"
        );
        let results = parse_results(&raw)?;
        let blocks = search_blocks(&tool_use_id, &query, results);
        let input_tokens = u64::try_from(super::content::token_count(request)).unwrap_or(u64::MAX);
        let output_tokens = blocks
            .iter()
            .map(|block| block.to_string().len())
            .sum::<usize>();
        if request.stream {
            Ok(streaming_server_response(
                &request.model,
                input_tokens,
                blocks,
            ))
        } else {
            Ok(anthropic_response(
                Segment {
                    blocks,
                    stop_reason: "end_turn",
                    usage: Usage {
                        input_tokens,
                        output_tokens: estimated_tokens(&"x".repeat(output_tokens)),
                    },
                },
                &request.model,
            ))
        }
    }
}

fn search_query(request: &MessagesRequest) -> Result<String> {
    let text = request
        .messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|message| message.get("content"))
        .map(content_text)
        .filter(|text| !text.trim().is_empty())
        .context("web_search handoff query is missing")?;
    let text = text.trim();
    Ok(text
        .strip_prefix("Perform a web search for the query:")
        .map(str::trim)
        .unwrap_or(text)
        .to_owned())
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                (item.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| item.get("text").and_then(Value::as_str))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn search_prompt(query: &str, request: &MessagesRequest) -> String {
    let restrictions = request
        .tools
        .iter()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some(WEB_SEARCH_TOOL))
        .map(|tool| {
            let mut values = Vec::new();
            if let Some(domains) = tool.get("allowed_domains") {
                values.push(format!("allowed_domains={domains}"));
            }
            if let Some(domains) = tool.get("blocked_domains") {
                values.push(format!("blocked_domains={domains}"));
            }
            values.join(", ")
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_default();
    format!("{WEB_SEARCH_MODEL_PROMPT}\nQuery: {query}\n{restrictions}")
}

fn parse_results(raw: &str) -> Result<Vec<Value>> {
    if let Some(value) = parse_json_value(raw) {
        return normalize_results(value);
    }
    let markdown = raw.lines().filter_map(markdown_result).collect::<Vec<_>>();
    if markdown.is_empty() {
        bail!("Claude Haiku returned non-JSON WebSearch output");
    }
    Ok(markdown)
}

fn normalize_results(value: Value) -> Result<Vec<Value>> {
    let items = value
        .as_array()
        .context("Claude Haiku WebSearch output must be a JSON array")?;
    if items.is_empty() {
        bail!("Claude Haiku WebSearch returned no results");
    }
    let mut results = Vec::with_capacity(items.len());
    for item in items {
        let title = item.get("title").and_then(Value::as_str).unwrap_or("");
        let url = item.get("url").and_then(Value::as_str).unwrap_or("");
        if title.trim().is_empty() || !(url.starts_with("https://") || url.starts_with("http://")) {
            bail!("Claude Haiku WebSearch returned an invalid result");
        }
        results.push(json!({
            "type":"web_search_result",
            "title":title,
            "url":url,
            "page_age":item.get("page_age").cloned().unwrap_or(Value::Null),
            "encrypted_content":item.get("encrypted_content").and_then(Value::as_str).unwrap_or("")
        }));
    }
    Ok(results)
}

fn parse_json_value(raw: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str(raw.trim()) {
        return Some(value);
    }
    for (start, _) in raw.match_indices('[') {
        for (relative_end, _) in raw[start..].match_indices(']') {
            let end = start + relative_end + 1;
            if let Ok(value) = serde_json::from_str(&raw[start..end]) {
                return Some(value);
            }
        }
    }
    None
}

fn markdown_result(line: &str) -> Option<Value> {
    let title_start = line.find('[')?;
    let title_end = line[title_start + 1..].find(']')? + title_start + 1;
    let url_start = line[title_end + 1..].find('(')? + title_end + 1;
    let url_end = line[url_start + 1..].find(')')? + url_start + 1;
    let title = line[title_start + 1..title_end].trim();
    let url = line[url_start + 1..url_end].trim();
    if title.is_empty() || !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    Some(json!({
        "type":"web_search_result",
        "title":title,
        "url":url,
        "page_age":Value::Null,
        "encrypted_content":""
    }))
}

fn search_blocks(tool_use_id: &str, query: &str, results: Vec<Value>) -> Vec<Value> {
    vec![
        json!({
            "type":"server_tool_use",
            "id":tool_use_id,
            "name":WEB_SEARCH_TOOL,
            "input":{"query":query},
            "caller":{"type":"direct"}
        }),
        json!({
            "type":"web_search_tool_result",
            "tool_use_id":tool_use_id,
            "content":results
        }),
    ]
}

fn streaming_server_response(model: &str, input_tokens: u64, blocks: Vec<Value>) -> Response<Body> {
    let (sender, receiver) = mpsc::channel::<Result<Bytes, Infallible>>(16);
    let model = model.to_owned();
    tokio::spawn(async move {
        let _ = sender
            .send(Ok(Bytes::from(message_start(&model, input_tokens))))
            .await;
        for (index, block) in blocks.iter().enumerate() {
            let _ = send_stream_frame(
                Some(&sender),
                "content_block_start",
                || json!({"type":"content_block_start","index":index,"content_block":block}),
            )
            .await;
            let _ = send_stream_frame(
                Some(&sender),
                "content_block_stop",
                || json!({"type":"content_block_stop","index":index}),
            )
            .await;
        }
        let _ = send_stream_frame(Some(&sender), "message_delta", || {
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":estimated_tokens(&blocks.iter().map(Value::to_string).collect::<String>())}})
        })
        .await;
        let _ = send_stream_frame(
            Some(&sender),
            "message_stop",
            || json!({"type":"message_stop"}),
        )
        .await;
    });
    streaming_sse_response(receiver)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{is_web_search_handoff, parse_results, search_query};
    use crate::anthropic::MessagesRequest;

    fn request(tool: serde_json::Value, content: &str) -> MessagesRequest {
        serde_json::from_value(json!({
            "model":"synthetic-model",
            "tools":[tool],
            "messages":[{"role":"user","content":content}]
        }))
        .expect("synthetic request")
    }

    #[test]
    fn recognizes_only_typed_web_search_handoffs() {
        assert!(is_web_search_handoff(&request(
            json!({"type":"web_search_20250305","name":"web_search"}),
            "synthetic query"
        )));
        assert!(!is_web_search_handoff(&request(
            json!({"type":"function","name":"web_search"}),
            "synthetic query"
        )));
    }

    #[test]
    fn extracts_the_query_from_a_server_handoff_prompt() {
        assert_eq!(
            search_query(&request(
                json!({"type":"web_search_20250305","name":"web_search"}),
                "Perform a web search for the query: synthetic query"
            ))
            .expect("query"),
            "synthetic query"
        );
    }

    #[test]
    fn rejects_empty_or_invalid_search_results() {
        assert!(parse_results("[]").is_err());
        assert!(parse_results("[{\"title\":\"missing url\"}]").is_err());
        assert!(
            parse_results("[{\"title\":\"Synthetic\",\"url\":\"https://example.test/result\"}]")
                .is_ok()
        );
    }

    #[test]
    fn accepts_search_json_wrapped_in_prose_or_markdown_sources() {
        assert!(parse_results(
            "The results are:\n```json\n[{\"title\":\"Synthetic\",\"url\":\"https://example.test/result\"}]\n```"
        )
        .is_ok());
        let markdown = parse_results("Sources:\n- [Synthetic](https://example.test/result)")
            .expect("markdown result");
        assert_eq!(markdown[0]["url"], "https://example.test/result");
    }
}
