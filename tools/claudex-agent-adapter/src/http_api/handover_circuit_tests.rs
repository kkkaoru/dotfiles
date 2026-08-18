use super::*;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

#[test]
fn first_failures_stay_closed_until_the_burst_limit() {
    let circuit = HandoverCircuit::default();
    assert!(!circuit.is_open("session-a"));
    assert!(!circuit.note_failure("session-a"));
    assert!(!circuit.note_failure("session-a"));
    assert!(
        circuit.note_failure("session-a"),
        "third failure in the window must open the circuit"
    );
    assert!(circuit.is_open("session-a"));
    assert!(
        circuit.note_failure("session-a"),
        "an open circuit stays open"
    );
}

#[test]
fn clear_closes_an_open_circuit_so_a_later_failure_starts_a_new_burst() {
    let circuit = HandoverCircuit::default();
    assert!(!circuit.note_failure("session-a"));
    assert!(!circuit.note_failure("session-a"));
    assert!(circuit.note_failure("session-a"));
    circuit.clear("session-a");
    assert!(!circuit.is_open("session-a"));
    assert!(
        !circuit.note_failure("session-a"),
        "a cleared circuit must not stay open on the next failure"
    );
}

#[test]
fn sibling_sessions_do_not_share_a_circuit() {
    let circuit = HandoverCircuit::default();
    assert!(!circuit.note_failure("session-a"));
    assert!(!circuit.note_failure("session-a"));
    assert!(circuit.note_failure("session-a"));
    assert!(!circuit.is_open("session-b"));
    assert!(!circuit.note_failure("session-b"));
}

#[test]
fn poisoned_session_state_fails_closed() {
    let circuit = Arc::new(HandoverCircuit::default());
    let poisoned = Arc::clone(&circuit);
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _guard = poisoned.sessions.lock().expect("unpoisoned sessions");
        panic!("poison handover circuit state");
    }));
    assert!(panic.is_err());

    assert!(
        circuit.note_failure("session-poisoned"),
        "a poisoned circuit must reject the failure conservatively"
    );
}

#[tokio::test]
async fn retry_response_is_503_with_retry_after_and_non_retryable_type() {
    let response = retry_response("canonical listen is unreachable".to_owned());
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(header::RETRY_AFTER)
            .map(|v| v.as_bytes()),
        Some(b"1".as_slice())
    );
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("retry body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json["error"]["type"], "invalid_request_error");
}

#[tokio::test]
async fn terminal_response_is_non_retryable() {
    let response = terminal_response("circuit open".to_owned());
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(is_retry_status(StatusCode::SERVICE_UNAVAILABLE));
    assert!(!is_retry_status(StatusCode::BAD_GATEWAY));
    assert!(!is_retry_status(StatusCode::BAD_REQUEST));
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("terminal body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json["error"]["type"], "invalid_request_error");
}
