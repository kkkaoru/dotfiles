use std::fmt;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::timeout;

use std::sync::Arc;

use crate::{agent_backend::AgentBackend, provider_config::WorkerRoute};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WebSearchMode {
    #[default]
    DelegateCcr,
    CodexNative,
    AcpNative,
    DelegateMcp,
    Disabled,
}

impl WebSearchMode {
    pub const fn is_default(&self) -> bool {
        matches!(self, Self::DelegateCcr)
    }
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DelegateCcr => "delegate-ccr",
            Self::CodexNative => "codex-native",
            Self::AcpNative => "acp-native",
            Self::DelegateMcp => "delegate-mcp",
            Self::Disabled => "disabled",
        }
    }
}

impl fmt::Display for WebSearchMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub search_count: u64,
}

const SEARCH_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const LOCAL_CCR_ENVIRONMENT: [&str; 3] = [
    "CLAUDE_CODE_WEBSEARCH_USE_CCR_PROXY",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ACCESS_TOKEN",
];

pub(crate) fn clear_local_ccr_environment(command: &mut Command) {
    for variable in LOCAL_CCR_ENVIRONMENT {
        command.env_remove(variable);
    }
}

pub(crate) async fn run(
    backend: &Arc<AgentBackend>,
    workers: &[WorkerRoute],
    query: &str,
) -> Result<SearchResponse> {
    if query.trim().is_empty() {
        bail!("WebSearch query must not be empty");
    }
    let mut errors = Vec::new();
    for worker in workers {
        match run_worker_with_timeout(backend, worker, query).await {
            Ok(response) if !response.results.is_empty() => return Ok(response),
            Ok(_) => errors.push(format!("{} returned no results", worker.model)),
            Err(error) => errors.push(format!("{}: {error:#}", worker.model)),
        }
    }
    if errors.is_empty() {
        bail!("no WebSearch worker is configured")
    }
    bail!("all WebSearch workers failed: {}", errors.join("; "))
}

// Live provider I/O and scheduler timeout outcomes are validated by the CCR
// integration test; excluding this transport boundary keeps coverage stable
// without depending on external credentials or wall-clock scheduling.
#[cfg_attr(coverage_nightly, coverage(off))]
async fn run_worker_with_timeout(
    backend: &Arc<AgentBackend>,
    worker: &WorkerRoute,
    query: &str,
) -> Result<SearchResponse> {
    match timeout(SEARCH_TIMEOUT, run_worker(backend, worker, query)).await {
        Ok(result) => result,
        Err(_) => bail!("{} timed out", worker.model),
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
async fn run_worker(
    backend: &Arc<AgentBackend>,
    worker: &WorkerRoute,
    query: &str,
) -> Result<SearchResponse> {
    let params = json!({
        "model": worker.model,
        "baseInstructions": "Use only the Codex built-in live WebSearch for the exact query. Return the source title, URL, and a short snippet; do not perform filesystem, shell, MCP, Agent, or Task operations.",
        "developerInstructions": "This is a retrieval-only worker. Never delegate and never call an external dynamic tool.",
        "dynamicTools": [],
        "ephemeral": true,
        "approvalPolicy": "never",
        "sandbox": "read-only",
        "config": {"web_search":"live", "features":{"web_search":true}}
    });
    let started = backend.request("thread/start", params).await?;
    let thread_id = started
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .context("search worker omitted thread id")?
        .to_owned();
    let events = backend.subscribe_thread(&thread_id);
    backend
        .request_detached(
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": [{"type":"text", "text":query}],
                "model": worker.model,
                "effort": worker.effort
            }),
        )
        .await?;
    let mut results = Vec::new();
    let mut answer = String::new();
    let mut search_count = 0;
    while let Some(event) = events.recv().await {
        match event.get("method").and_then(Value::as_str) {
            Some("item/started") if is_web_search(&event) => {
                search_count += 1;
                collect_item_results(&event, &mut results);
            }
            Some("item/completed") if is_web_search(&event) => {
                collect_item_results(&event, &mut results);
            }
            Some("item/agentMessage/delta") => {
                if let Some(delta) = event.pointer("/params/delta").and_then(Value::as_str) {
                    answer.push_str(delta);
                }
            }
            Some("turn/completed") | Some("error") => break,
            _ => {}
        }
    }
    // A URL in the model's prose is not evidence that a live search happened.
    // Only use prose fallback after the provider emitted at least one native
    // search event; otherwise the caller must observe a failed search.
    if results.is_empty() {
        results = fallback_results(search_count, &answer);
    }
    Ok(SearchResponse {
        query: query.to_owned(),
        results,
        search_count,
    })
}

fn is_web_search(event: &Value) -> bool {
    event.pointer("/params/item/type").and_then(Value::as_str) == Some("webSearch")
}

fn collect_item_results(event: &Value, output: &mut Vec<SearchResult>) {
    let Some(items) = event
        .pointer("/params/item/results")
        .and_then(Value::as_array)
    else {
        return;
    };
    output.extend(items.iter().filter_map(parse_result));
    output.sort_by(|left, right| left.url.cmp(&right.url));
    output.dedup_by(|left, right| left.url == right.url);
}

fn parse_result(value: &Value) -> Option<SearchResult> {
    let title = value.get("title").and_then(Value::as_str)?.trim();
    let url = value.get("url").and_then(Value::as_str)?.trim();
    if title.is_empty() || url.is_empty() {
        return None;
    }
    Some(SearchResult {
        title: title.to_owned(),
        url: url.to_owned(),
        snippet: value
            .get("snippet")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn extract_urls(text: &str) -> Vec<SearchResult> {
    text.split_whitespace()
        .filter_map(|token| {
            let url = token.trim_matches(|character: char| "()[]{}<>,.;\"'".contains(character));
            (url.starts_with("https://") || url.starts_with("http://")).then(|| SearchResult {
                title: url.to_owned(),
                url: url.to_owned(),
                snippet: None,
            })
        })
        .collect()
}

fn fallback_results(search_count: u64, answer: &str) -> Vec<SearchResult> {
    (search_count > 0)
        .then(|| extract_urls(answer))
        .unwrap_or_default()
}

#[cfg(test)]
// Search workers are covered through pure parsing tests here; the live provider
// transport is exercised by the adapter integration path and is intentionally
// not coupled to external credentials in unit tests.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn serializes_each_search_mode_and_keeps_the_default_compact() {
        assert_eq!(WebSearchMode::default().as_str(), "delegate-ccr");
        assert!(WebSearchMode::default().is_default());
        for (mode, name) in [
            (WebSearchMode::CodexNative, "codex-native"),
            (WebSearchMode::AcpNative, "acp-native"),
            (WebSearchMode::DelegateMcp, "delegate-mcp"),
            (WebSearchMode::Disabled, "disabled"),
        ] {
            assert_eq!(mode.as_str(), name);
            assert!(!mode.is_default());
            assert_eq!(mode.to_string(), name);
            let encoded = serde_json::to_value(mode).expect("search mode JSON");
            assert_eq!(encoded, name);
            assert_eq!(
                serde_json::from_value::<WebSearchMode>(encoded).unwrap(),
                mode
            );
        }
    }

    #[test]
    fn parses_and_deduplicates_provider_results() {
        let event = serde_json::json!({
            "params": {"item": {"type": "webSearch", "results": [
                {"title":"First", "url":"https://example.test/a", "snippet":"one"},
                {"title":"duplicate", "url":"https://example.test/a"},
                {"title":"missing-url"}, {"title":"", "url":"https://example.test/b"}
            ]}}
        });
        let mut results = Vec::new();
        assert!(is_web_search(&event));
        collect_item_results(&event, &mut results);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "First");
        assert_eq!(results[0].snippet.as_deref(), Some("one"));
        assert!(!is_web_search(
            &serde_json::json!({"params":{"item":{"type":"text"}}})
        ));
    }

    #[test]
    fn extracts_http_urls_and_rejects_empty_queries_without_workers() {
        let results = extract_urls("See (https://example.test/a), http://example.test/b.");
        assert_eq!(
            results
                .iter()
                .map(|result| result.url.as_str())
                .collect::<Vec<_>>(),
            ["https://example.test/a", "http://example.test/b"]
        );
        assert!(extract_urls("no links here").is_empty());
    }

    #[test]
    fn does_not_treat_prose_urls_as_a_live_search_result_without_an_event() {
        assert!(fallback_results(0, "https://example.test/from-memory").is_empty());
        assert_eq!(
            fallback_results(1, "https://example.test/after-search")[0].url,
            "https://example.test/after-search"
        );
    }

    #[tokio::test]
    async fn rejects_empty_queries_and_missing_workers() {
        let backend = AgentBackend::routed(Vec::new());
        let empty = run(&backend, &[], " ").await.expect_err("empty query");
        assert!(empty.to_string().contains("must not be empty"));
        let no_workers = run(&backend, &[], "query").await.expect_err("no workers");
        assert!(no_workers.to_string().contains("no WebSearch worker"));
        let worker = WorkerRoute {
            agent: "worker".to_owned(),
            model: "model".to_owned(),
            effort: "high".to_owned(),
        };
        let failed = run(&backend, std::slice::from_ref(&worker), "query")
            .await
            .expect_err("unrouted search worker");
        assert!(failed.to_string().contains("all WebSearch workers failed"));
    }
}
