use std::{
    pin::Pin,
    task::{Context as TaskContext, Poll},
    time::Instant,
};

use axum::{
    body::{BodyDataStream, Bytes},
    http::StatusCode,
};
use tokio_stream::Stream;
use tracing::Span;

pub(super) struct RequestTrace {
    pub(super) request_id: String,
    pub(super) session_id: String,
    pub(super) agent_id: String,
    pub(super) parent_agent_id: String,
    pub(super) method: String,
    pub(super) path: String,
    pub(super) status: StatusCode,
    pub(super) started: Instant,
    pub(super) span: Span,
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

pub(super) struct TracedBodyStream {
    pub(super) inner: BodyDataStream,
    pub(super) trace: Option<RequestTrace>,
    pub(super) body_bytes: usize,
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
