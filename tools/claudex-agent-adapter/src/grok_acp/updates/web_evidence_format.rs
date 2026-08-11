use agent_client_protocol as acp;
use serde_json::{Value, json};

use super::{MAX_RESULT_SUMMARY_CHARS, MAX_SOURCE_URLS, WebOperation};

pub(in crate::grok_acp::updates) fn web_operation(
    title: &str,
    kind: Option<acp::ToolKind>,
    raw_input: Option<&Value>,
) -> Option<WebOperation> {
    let normalized = normalized_title(title);
    let query = string_argument(raw_input, "query");
    let url = string_argument(raw_input, "url").filter(|url| is_http_url(url));
    let kind = match normalized.as_str() {
        "websearch" | "searchtheweb" => "web_search",
        "webfetch" => "web_fetch",
        _ if kind == Some(acp::ToolKind::Fetch) && url.is_some() => "web_fetch",
        // ACP `Search` includes repository and workspace search. Never infer a
        // live retrieval from it without an explicit WebSearch tool identity.
        _ => return None,
    };
    Some(WebOperation { kind, query, url })
}

pub(in crate::grok_acp::updates) fn completion_evidence(
    operation: WebOperation,
    raw_output: Option<Value>,
    content: Option<&Vec<acp::ToolCallContent>>,
) -> Option<Value> {
    let output = provider_output(raw_output, content);
    meaningful_provider_output(&output).then_some(())?;
    let mut source_urls = operation.url.into_iter().collect::<Vec<_>>();
    source_urls.extend(extract_source_urls(&output));
    source_urls.dedup();
    source_urls.truncate(MAX_SOURCE_URLS);
    (!source_urls.is_empty()).then_some(())?;
    let mut evidence = json!({
        "provider": "acp",
        "provenance": "provider-native-tool-completion",
        "kind": operation.kind,
        "evidence_class": evidence_class(operation.kind),
        "status": "completed",
        "verified": true,
        "result_summary": summary(&output),
        "source_urls": source_urls,
    });
    if let Some(query) = operation.query {
        evidence["query"] = json!(query);
    }
    Some(evidence)
}

pub(super) fn evidence_class(kind: &str) -> &'static str {
    match kind {
        "web_fetch" => "fetch_verified",
        _ => "search_result_only",
    }
}

pub(super) fn normalized_title(title: &str) -> String {
    let head = title.split(':').next().unwrap_or(title).trim();
    let title = head.strip_prefix("Using ").unwrap_or(head);
    title
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

pub(super) fn string_argument(input: Option<&Value>, key: &str) -> Option<String> {
    input
        .and_then(Value::as_object)
        .and_then(|input| input.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(super) fn is_http_url(url: &str) -> bool {
    matches!(url.split_once("://"), Some(("http" | "https", host)) if !host.is_empty())
}

pub(super) fn provider_output(
    raw_output: Option<Value>,
    content: Option<&Vec<acp::ToolCallContent>>,
) -> String {
    let output = raw_output.map_or_else(String::new, |output| match output {
        Value::String(text) => text,
        value => value.to_string(),
    });
    let content = content
        .into_iter()
        .flatten()
        .filter_map(|item| match item {
            acp::ToolCallContent::Content(block) => match &block.content {
                acp::ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    match (output.is_empty(), content.is_empty()) {
        (false, false) if output != content => format!("{output}\n{content}"),
        (false, _) => output,
        (_, false) => content,
        _ => String::new(),
    }
}

pub(super) fn summary(output: &str) -> String {
    let compact = output.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = compact.chars();
    let summary = chars
        .by_ref()
        .take(MAX_RESULT_SUMMARY_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        format!("{summary}…")
    } else {
        summary
    }
}

pub(super) fn meaningful_provider_output(output: &str) -> bool {
    !matches!(output.trim(), "" | "null" | "{}" | "[]")
}

pub(super) fn extract_source_urls(output: &str) -> Vec<String> {
    output
        .split_whitespace()
        .filter_map(|part| {
            let start = part.find("https://").or_else(|| part.find("http://"))?;
            let url = part[start..].trim_matches(|character: char| {
                !character.is_ascii_graphic() || ",.;:!?)]}>\"'".contains(character)
            });
            is_http_url(url).then(|| url.to_owned())
        })
        .collect()
}
