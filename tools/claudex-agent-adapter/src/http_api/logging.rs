use std::time::Instant;

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderValue, Response},
    middleware::Next,
};
use tracing::Instrument;
use uuid::Uuid;

use super::request_identity;

#[path = "logging_trace.rs"]
mod logging_trace;
use logging_trace::{RequestTrace, TracedBodyStream};

const CLAUDEX_REQUEST_ID_HEADER: &str = "x-claudex-request-id";
const CCR_SESSION_PATH_PREFIX: &str = "/v1/code/sessions/";
const CCR_SESSION_PATH_SUFFIX: &str = "/worker/web-search";

pub(super) fn path_session_id(path: &str) -> Option<&str> {
    let session_id = path
        .strip_prefix(CCR_SESSION_PATH_PREFIX)?
        .strip_suffix(CCR_SESSION_PATH_SUFFIX)?;
    (!session_id.is_empty() && !session_id.contains('/')).then_some(session_id)
}

pub(super) async fn trace_http_request(request: Request, next: Next) -> Response<Body> {
    let request_id = Uuid::new_v4().to_string();
    let method = request.method().to_string();
    let path = request.uri().path().to_owned();
    let started = Instant::now();
    let identity = request_identity(request.headers());
    let (mut session_id, agent_id, parent_agent_id, identity_error) = match identity {
        Ok(identity) => (
            identity.session_id().unwrap_or_default().to_owned(),
            identity.agent_id().unwrap_or_default().to_owned(),
            identity.parent_agent_id().unwrap_or_default().to_owned(),
            String::new(),
        ),
        Err(error) => (
            String::new(),
            String::new(),
            String::new(),
            error.to_string(),
        ),
    };
    if session_id.is_empty() {
        session_id = path_session_id(&path).unwrap_or_default().to_owned();
    }
    let span = tracing::info_span!(
        target: "claudex.http",
        "http_request",
        request_id = %request_id,
        session_id = %session_id,
        agent_id = %agent_id,
        parent_agent_id = %parent_agent_id,
        method = %method,
        path = %path,
    );
    span.in_scope(|| {
        tracing::info!(
            target: "claudex.http",
            log_event = "http_request_start",
            request_id = %request_id,
            session_id = %session_id,
            agent_id = %agent_id,
            parent_agent_id = %parent_agent_id,
            method = %method,
            path = %path,
            identity_error = %identity_error,
            "HTTP request started"
        );
    });
    let response = next.run(request).instrument(span.clone()).await;
    let status = response.status();
    let mut response = response;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(
            axum::http::header::HeaderName::from_static(CLAUDEX_REQUEST_ID_HEADER),
            value,
        );
    }
    response.map(|body| {
        Body::from_stream(TracedBodyStream {
            inner: body.into_data_stream(),
            trace: Some(RequestTrace {
                request_id,
                session_id,
                agent_id,
                parent_agent_id,
                method,
                path,
                status,
                started,
                span,
            }),
            body_bytes: 0,
        })
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "logging_tests.rs"]
mod tests;
