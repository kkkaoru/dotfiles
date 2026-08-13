use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;

use crate::anthropic::{Bridge, error_response};
use crate::web_search::human_web_search_error;

pub(super) const CCR_KEEPALIVE: &str = " ";

#[derive(Debug, Default, Deserialize)]
pub(super) struct CcrWebSearchRequest {
    query: String,
    #[serde(default)]
    allowed_domains: Vec<String>,
    #[serde(default)]
    blocked_domains: Vec<String>,
}

pub(super) async fn ccr_web_search(
    State(bridge): State<Arc<Bridge>>,
    Path(session_id): Path<String>,
    Json(request): Json<CcrWebSearchRequest>,
) -> axum::response::Response<axum::body::Body> {
    if session_id.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            anyhow::anyhow!("session_id is empty"),
        );
    }
    stream_ccr_web_search(bridge, request).await
}

async fn stream_ccr_web_search(
    bridge: Arc<Bridge>,
    request: CcrWebSearchRequest,
) -> axum::response::Response<Body> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, std::io::Error>>(4);
    let _ = tx.send(Ok(CCR_KEEPALIVE.to_owned())).await;
    tokio::spawn(async move {
        let payload = match search_payload(bridge, request).await {
            Ok(value) => value,
            Err(error) => failed_search_payload(&error),
        };
        let _ = tx.send(Ok(payload.to_string())).await;
    });
    let stream = ReceiverStream::new(rx);
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        Body::from_stream(stream),
    )
        .into_response()
}

async fn search_payload(bridge: Arc<Bridge>, request: CcrWebSearchRequest) -> anyhow::Result<Value> {
    let mut response = bridge.run_web_search(&request.query).await?;
    response.results.retain(|result| {
        domain_allowed(
            &result.url,
            &request.allowed_domains,
            &request.blocked_domains,
        )
    });
    response.search_count = u64::try_from(response.results.len()).unwrap_or(u64::MAX);
    Ok(json!({"results": response.results, "error": Value::Null}))
}

pub(super) fn failed_search_payload(error: &anyhow::Error) -> Value {
    json!({
        "results": [],
        "error": human_web_search_error(error)
    })
}

pub(super) fn domain_allowed(url: &str, allowed: &[String], blocked: &[String]) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return false;
    }
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    let matches = |domain: &String| {
        let domain = domain.trim_start_matches(".").to_ascii_lowercase();
        host == domain || host.ends_with(&format!(".{domain}"))
    };
    !blocked.iter().any(matches) && (allowed.is_empty() || allowed.iter().any(matches))
}
