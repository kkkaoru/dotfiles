use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;

/// Burst window for same-session handover failures. This is a retry-storm
/// detector, not a product hang timeout.
const BURST_WINDOW: Duration = Duration::from_secs(2);
const BURST_LIMIT: u32 = 3;
const OPEN_TTL: Duration = Duration::from_secs(2);
const RETRY_AFTER_SECS: HeaderValue = HeaderValue::from_static("1");

#[derive(Default)]
pub(super) struct HandoverCircuit {
    sessions: Mutex<HashMap<String, SessionBurst>>,
}

struct SessionBurst {
    first: Instant,
    opened_at: Option<Instant>,
    count: u32,
    open: bool,
}

impl HandoverCircuit {
    pub(super) fn is_open(&self, session_id: &str) -> bool {
        let Ok(mut sessions) = self.sessions.lock() else {
            return false;
        };
        let Some(burst) = sessions.get_mut(session_id) else {
            return false;
        };
        if !burst.open {
            return false;
        }
        let opened_at = burst.opened_at.unwrap_or(burst.first);
        if Instant::now().duration_since(opened_at) > OPEN_TTL {
            sessions.remove(session_id);
            return false;
        }
        true
    }

    pub(super) fn clear(&self, session_id: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(session_id);
        }
    }

    /// Record a handover proxy failure. Returns true when the circuit is open.
    pub(super) fn note_failure(&self, session_id: &str) -> bool {
        let Ok(mut sessions) = self.sessions.lock() else {
            return true;
        };
        let now = Instant::now();
        let burst = sessions
            .entry(session_id.to_owned())
            .or_insert_with(|| SessionBurst {
                first: now,
                opened_at: None,
                count: 0,
                open: false,
            });
        if burst.open {
            let opened_at = burst.opened_at.unwrap_or(burst.first);
            if now.duration_since(opened_at) > OPEN_TTL {
                burst.first = now;
                burst.opened_at = None;
                burst.count = 0;
                burst.open = false;
            } else {
                return true;
            }
        }
        if now.duration_since(burst.first) > BURST_WINDOW {
            burst.first = now;
            burst.count = 0;
        }
        burst.count = burst.count.saturating_add(1);
        if burst.count >= BURST_LIMIT {
            burst.open = true;
            burst.opened_at = Some(now);
        }
        burst.open
    }
}

pub(super) fn retry_response(message: String) -> Response {
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(invalid_request_body(message)),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, RETRY_AFTER_SECS);
    response
}

pub(super) fn terminal_response(message: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(invalid_request_body(message))).into_response()
}

fn invalid_request_body(message: String) -> serde_json::Value {
    json!({
        "type": "error",
        "error": {
            "type": "invalid_request_error",
            "message": message,
        }
    })
}

pub(super) fn is_retry_status(status: StatusCode) -> bool {
    status == StatusCode::SERVICE_UNAVAILABLE
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "handover_circuit_tests.rs"]
mod tests;
