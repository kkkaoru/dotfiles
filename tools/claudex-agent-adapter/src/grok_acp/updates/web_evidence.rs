//! Correlate explicit provider-native web tools with their completion updates.
//!
//! ACP tool categories are intentionally broad: `Search` can mean repository
//! search. This tracker stores only an explicit WebSearch/WebFetch candidate and
//! emits it once the provider reports that exact call as completed.

use std::collections::HashMap;
use std::sync::Mutex;

use agent_client_protocol::{self as acp};
use serde_json::{Value, json};

const MAX_TRACKED_CALLS: usize = 256;
const MAX_RESULT_SUMMARY_CHARS: usize = 320;
const MAX_SOURCE_URLS: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WebOperation {
    pub(super) kind: &'static str,
    pub(super) query: Option<String>,
    pub(super) url: Option<String>,
}

#[derive(Default)]
pub(super) struct ProviderWebEvidence {
    calls: Mutex<HashMap<(String, String), TrackedOperation>>,
}

#[derive(Clone, Debug)]
struct TrackedOperation {
    operation: WebOperation,
    completed: bool,
}

impl ProviderWebEvidence {
    pub(super) fn record(&self, session_id: &str, call_id: &str, operation: WebOperation) {
        let Ok(mut calls) = self.calls.lock() else {
            return;
        };
        let key = (session_id.to_owned(), call_id.to_owned());
        if calls.contains_key(&key) || calls.len() >= MAX_TRACKED_CALLS {
            return;
        }
        calls.insert(
            key,
            TrackedOperation {
                operation,
                completed: false,
            },
        );
    }

    pub(super) fn completion_candidate(
        &self,
        session_id: &str,
        call_id: &str,
        direct_operation: Option<WebOperation>,
    ) -> Option<WebOperation> {
        let Ok(mut calls) = self.calls.lock() else {
            return None;
        };
        let key = (session_id.to_owned(), call_id.to_owned());
        if let Some(operation) = direct_operation
            && !calls.contains_key(&key)
            && calls.len() < MAX_TRACKED_CALLS
        {
            calls.insert(
                key.clone(),
                TrackedOperation {
                    operation,
                    completed: false,
                },
            );
        }
        let tracked = calls.get(&key)?;
        if tracked.completed {
            return None;
        }
        Some(tracked.operation.clone())
    }

    pub(super) fn mark_completed(&self, session_id: &str, call_id: &str) -> bool {
        let Ok(mut calls) = self.calls.lock() else {
            return false;
        };
        let key = (session_id.to_owned(), call_id.to_owned());
        let Some(tracked) = calls.get_mut(&key) else {
            return false;
        };
        if tracked.completed {
            return false;
        }
        tracked.completed = true;
        true
    }

    #[cfg(test)]
    pub(super) fn complete(
        &self,
        session_id: &str,
        call_id: &str,
        direct_operation: Option<WebOperation>,
    ) -> Option<WebOperation> {
        let operation = self.completion_candidate(session_id, call_id, direct_operation)?;
        self.mark_completed(session_id, call_id)
            .then_some(operation)
    }

    pub(super) fn clear(&self, session_id: &str) {
        let Ok(mut calls) = self.calls.lock() else {
            return;
        };
        calls.retain(|(session, _), _| session != session_id);
    }
}

pub(super) fn web_operation(
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

pub(super) fn completion_evidence(
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

fn evidence_class(kind: &str) -> &'static str {
    match kind {
        "web_fetch" => "fetch_verified",
        _ => "search_result_only",
    }
}

fn normalized_title(title: &str) -> String {
    let head = title.split(':').next().unwrap_or(title).trim();
    let title = head.strip_prefix("Using ").unwrap_or(head);
    title
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

fn string_argument(input: Option<&Value>, key: &str) -> Option<String> {
    input
        .and_then(Value::as_object)
        .and_then(|input| input.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn is_http_url(url: &str) -> bool {
    matches!(url.split_once("://"), Some(("http" | "https", host)) if !host.is_empty())
}

fn provider_output(
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

fn summary(output: &str) -> String {
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

fn meaningful_provider_output(output: &str) -> bool {
    !matches!(output.trim(), "" | "null" | "{}" | "[]")
}

fn extract_source_urls(output: &str) -> Vec<String> {
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

#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    include!("web_evidence_tests.rs");
}
