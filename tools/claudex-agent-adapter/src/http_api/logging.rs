use std::{
    pin::Pin,
    task::{Context as TaskContext, Poll},
    time::Instant,
};

use axum::{
    body::{Body, BodyDataStream, Bytes},
    extract::Request,
    http::{HeaderValue, Response, StatusCode},
    middleware::Next,
};
use tokio_stream::Stream;
use tracing::{Instrument, Span};
use uuid::Uuid;

use super::request_identity;

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

struct RequestTrace {
    request_id: String,
    session_id: String,
    agent_id: String,
    parent_agent_id: String,
    method: String,
    path: String,
    status: StatusCode,
    started: Instant,
    span: Span,
}

impl RequestTrace {
    fn finish(self, outcome: &'static str, body_bytes: usize) {
        let duration_ms = self.started.elapsed().as_millis();
        if outcome == "completed" {
            self.log_completed(duration_ms, body_bytes);
        } else {
            self.log_abnormal(outcome, duration_ms, body_bytes);
        }
    }

    fn log_completed(self, duration_ms: u128, body_bytes: usize) {
        let RequestTrace {
            request_id,
            session_id,
            agent_id,
            parent_agent_id,
            method,
            path,
            status,
            span,
            ..
        } = self;
        span.in_scope(|| {
            tracing::info!(
                target: "claudex.http",
                log_event = "http_request_end",
                request_id = %request_id,
                session_id = %session_id,
                agent_id = %agent_id,
                parent_agent_id = %parent_agent_id,
                method = %method,
                path = %path,
                status = status.as_u16(),
                duration_ms,
                body_bytes,
                outcome = "completed",
                "HTTP request completed"
            );
        });
    }

    fn log_abnormal(self, outcome: &'static str, duration_ms: u128, body_bytes: usize) {
        let RequestTrace {
            request_id,
            session_id,
            agent_id,
            parent_agent_id,
            method,
            path,
            status,
            span,
            ..
        } = self;
        span.in_scope(|| {
            tracing::warn!(
                target: "claudex.http",
                log_event = "http_request_end",
                request_id = %request_id,
                session_id = %session_id,
                agent_id = %agent_id,
                parent_agent_id = %parent_agent_id,
                method = %method,
                path = %path,
                status = status.as_u16(),
                duration_ms,
                body_bytes,
                outcome,
                "HTTP request body ended abnormally"
            );
        });
    }
}

struct TracedBodyStream {
    inner: BodyDataStream,
    trace: Option<RequestTrace>,
    body_bytes: usize,
}

impl Stream for TracedBodyStream {
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let item = Pin::new(&mut self.inner).poll_next(cx);
        let outcome = match &item {
            Poll::Ready(Some(Ok(bytes))) => {
                self.body_bytes += bytes.len();
                None
            }
            Poll::Ready(None) => Some("completed"),
            Poll::Ready(Some(Err(_))) => Some("body_error"),
            Poll::Pending => None,
        };
        if let Some(outcome) = outcome {
            self.finish_trace(outcome);
        }
        item
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl TracedBodyStream {
    fn finish_trace(&mut self, outcome: &'static str) {
        let Some(trace) = self.trace.take() else {
            return;
        };
        trace.finish(outcome, self.body_bytes);
    }
}

impl Drop for TracedBodyStream {
    fn drop(&mut self) {
        self.finish_trace("client_disconnect");
    }
}

#[cfg(test)]
mod tests {
    use super::path_session_id;

    #[test]
    fn extracts_only_the_expected_ccr_session_path_shape() {
        assert_eq!(
            path_session_id("/v1/code/sessions/session-123/worker/web-search"),
            Some("session-123")
        );
        assert_eq!(
            path_session_id("/v1/code/sessions/session-123/worker/web-search/extra"),
            None
        );
        assert_eq!(
            path_session_id("/v1/code/sessions/a/b/worker/web-search"),
            None
        );
        assert_eq!(path_session_id("/health"), None);
    }
}
