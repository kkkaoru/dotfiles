use std::future::Future;

use anyhow::{Result, bail};
use tokio::task::JoinSet;

use super::SearchResponse;

pub(in crate::web_search) async fn first_nonempty_response<F>(
    query: &str,
    workers: impl IntoIterator<Item = F>,
) -> Result<SearchResponse>
where
    F: Future<Output = Result<SearchResponse>> + Send + 'static,
{
    let mut set = JoinSet::new();
    let mut started = 0usize;
    for worker in workers {
        started += 1;
        set.spawn(worker);
    }
    if started == 0 {
        bail!("no WebSearch worker is configured");
    }
    drain_first_nonempty(query, set).await
}

async fn drain_first_nonempty(
    query: &str,
    mut set: JoinSet<Result<SearchResponse>>,
) -> Result<SearchResponse> {
    let mut errors = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(response)) if !response.results.is_empty() => {
                set.abort_all();
                return Ok(response);
            }
            Ok(Ok(_)) => errors.push(format!("{query} worker returned no results")),
            Ok(Err(error)) => errors.push(error.to_string()),
            Err(error) => errors.push(format!("web search worker join failed: {error}")),
        }
    }
    if errors.is_empty() {
        bail!("no WebSearch worker is configured");
    }
    bail!("all WebSearch workers failed: {}", errors.join("; "))
}

pub(crate) fn human_web_search_error(error: &anyhow::Error) -> String {
    human_web_search_error_text(&error.to_string())
}

pub(crate) fn human_web_search_error_text(detail: &str) -> String {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        return "WebSearch timed out".to_owned();
    }
    if lower.contains("no websearch worker") {
        return "No WebSearch worker is configured".to_owned();
    }
    if lower.contains("must not be empty") {
        return "WebSearch query must not be empty".to_owned();
    }
    let cleaned = detail
        .replace("PROXY_TRANSPORT", "")
        .replace("ECONNABORTED", "")
        .replace("  ", " ");
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "WebSearch failed".to_owned()
    } else {
        format!("WebSearch failed: {trimmed}")
    }
}
