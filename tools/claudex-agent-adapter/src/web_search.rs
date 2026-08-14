use std::{future::Future, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::timeout;

use crate::{agent_backend::AgentBackend, app_server::ThreadEvents, provider_config::WorkerRoute};

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
    let backend = backend.clone();
    let query_owned = query.to_owned();
    let jobs = workers.iter().cloned().map(move |worker| {
        let backend = backend.clone();
        let query = query_owned.clone();
        async move { run_worker_with_timeout(&backend, &worker, &query).await }
    });
    first_nonempty_response(query, jobs).await
}

// Live provider I/O is validated by the CCR integration test. Event decisions
// and scheduler timeout outcomes use injected futures below for deterministic
// unit coverage without external credentials or wall-clock waits.
mod mode;
// Keep parse mapped under coverage-branch: coverage(off) makes llvm omit the
// file while expected_production_files still requires it (≥95% gate).
mod parse;
mod race;
pub use mode::WebSearchMode;
use parse::{append_answer_delta, collect_item_results, fallback_results, is_web_search};
#[cfg(test)]
use parse::{extract_urls, parse_result};
use race::first_nonempty_response;
pub(crate) use race::human_web_search_error;

async fn run_worker_with_timeout(
    backend: &Arc<AgentBackend>,
    worker: &WorkerRoute,
    query: &str,
) -> Result<SearchResponse> {
    wait_for_worker(worker, SEARCH_TIMEOUT, run_worker(backend, worker, query)).await
}

async fn wait_for_worker<F>(
    worker: &WorkerRoute,
    duration: Duration,
    response: F,
) -> Result<SearchResponse>
where
    F: Future<Output = Result<SearchResponse>>,
{
    match timeout(duration, response).await {
        Ok(result) => result,
        Err(_) => bail!("{} timed out", worker.model),
    }
}

async fn run_worker(
    backend: &Arc<AgentBackend>,
    worker: &WorkerRoute,
    query: &str,
) -> Result<SearchResponse> {
    run_worker_with_events(query, start_worker(backend, worker, query)).await
}

async fn start_worker(
    backend: &Arc<AgentBackend>,
    worker: &WorkerRoute,
    query: &str,
) -> Result<ThreadEvents> {
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
    Ok(events)
}

async fn run_worker_with_events<F>(query: &str, start: F) -> Result<SearchResponse>
where
    F: Future<Output = Result<ThreadEvents>>,
{
    let events = start.await?;
    collect_worker_response(query, &events).await
}

async fn collect_worker_response(query: &str, events: &ThreadEvents) -> Result<SearchResponse> {
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
