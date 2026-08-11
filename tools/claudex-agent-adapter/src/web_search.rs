use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::timeout;

use std::sync::Arc;

use crate::{agent_backend::AgentBackend, provider_config::WorkerRoute};


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
mod parse;
mod mode;
pub use mode::WebSearchMode;
use parse::{append_answer_delta, collect_item_results, fallback_results, is_web_search};
#[cfg(test)]
use parse::{extract_urls, parse_result};

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
    backend.ensure_thread_ready(&thread_id).await?;
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
            Some("item/agentMessage/delta") => append_answer_delta(&event, &mut answer),
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "web_search_tests.rs"]
mod tests;
