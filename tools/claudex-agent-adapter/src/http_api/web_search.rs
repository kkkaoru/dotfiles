use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::anthropic::{Bridge, error_response};

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
    match bridge.run_web_search(&request.query).await {
        Ok(mut response) => {
            response.results.retain(|result| {
                domain_allowed(
                    &result.url,
                    &request.allowed_domains,
                    &request.blocked_domains,
                )
            });
            response.search_count = u64::try_from(response.results.len()).unwrap_or(u64::MAX);
            Json(json!({"results": response.results, "error": Value::Null})).into_response()
        }
        Err(error) => error_response(StatusCode::BAD_GATEWAY, error),
    }
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
