use std::{path::PathBuf, time::Duration};

use super::*;

fn gateway() -> PiGateway {
    PiGateway {
        provider: "provider".to_owned(),
        model_id: "model".to_owned(),
        process: tokio::sync::Mutex::new(None),
        directory: PathBuf::new(),
        socket: PathBuf::new(),
        token: "token".to_owned(),
        events: Arc::new(ThreadEventDispatcher::default()),
        active: Arc::new(Mutex::new(HashMap::new())),
        pending_request_ids: Arc::new(Mutex::new(HashMap::new())),
        alive: AtomicBool::new(true),
    }
}

#[tokio::test]
async fn missing_extensions_identify_the_selected_pi_route() {
    let root = tempfile::tempdir().expect("extension fixture");
    let missing = root.path().join("missing.ts");
    let error = PiGateway::spawn("cursor", "auto", &[missing.display().to_string()])
        .await
        .err()
        .expect("missing extension");
    let message = format!("{error:#}");
    assert!(message.contains("provider `cursor` model `auto`"));
    assert!(message.contains(&missing.display().to_string()));
}

#[tokio::test]
async fn correlates_consecutive_turn_subscribers_by_unique_request_id() {
    let gateway = gateway();
    let first_events = gateway.subscribe_thread("session-thread");
    let second_events = gateway.subscribe_thread("session-thread");
    let first_id = gateway
        .take_reserved_request_id("session-thread")
        .expect("first reservation");
    let second_id = gateway
        .take_reserved_request_id("session-thread")
        .expect("second reservation");
    assert_ne!(first_id, second_id);
    assert!(gateway.pending_request_ids.lock().unwrap().is_empty());

    drop(first_events);
    gateway.events.dispatch_to(
        &first_id,
        json!({
            "method":"turn/completed",
            "params":{"threadId":"session-thread","turn":{"status":"completed"}}
        }),
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), second_events.recv())
            .await
            .is_err(),
        "a late terminal event from the first turn must not reach the second subscriber"
    );

    gateway.events.dispatch_to(
        &second_id,
        json!({
            "method":"turn/completed",
            "params":{"threadId":"session-thread","turn":{"status":"completed"}}
        }),
    );
    assert_eq!(
        second_events.recv().await.expect("second terminal event")["params"]["threadId"],
        "session-thread"
    );
}

#[test]
fn dropping_a_subscriber_releases_only_its_pending_reservation() {
    let gateway = gateway();
    let abandoned = gateway.subscribe_thread("session-thread");
    let retained = gateway.subscribe_thread("session-thread");
    assert_eq!(
        gateway.pending_request_ids.lock().unwrap()["session-thread"].len(),
        2
    );
    drop(abandoned);
    assert_eq!(
        gateway.pending_request_ids.lock().unwrap()["session-thread"].len(),
        1
    );
    drop(retained);
    assert!(gateway.pending_request_ids.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_turn_start_releases_reservation_when_subscriber_drops() {
    let gateway = Arc::new(gateway());
    let events = gateway.subscribe_thread("session-thread");
    let error = gateway
        .start_turn(json!({"threadId":"session-thread"}))
        .await
        .expect_err("missing Claudex request");
    assert!(error.to_string().contains("omitted claudexRequest"));
    drop(events);
    assert!(gateway.pending_request_ids.lock().unwrap().is_empty());
}

#[test]
fn rejects_turn_start_without_a_reserved_subscriber() {
    let gateway = gateway();
    let error = gateway
        .take_reserved_request_id("missing")
        .expect_err("missing reservation");
    assert!(error.to_string().contains("no reserved subscriber"));
}

#[tokio::test]
async fn cancellation_remains_keyed_by_session_thread() {
    let gateway = gateway();
    let (cancel, mut cancelled) = mpsc::unbounded_channel();
    gateway.active.lock().unwrap().insert(
        "session-thread".to_owned(),
        ActiveTurn {
            request_id: "request-id".to_owned(),
            cancel,
        },
    );
    gateway
        .cancel_turn("session-thread")
        .expect("cancel active Pi turn");
    assert_eq!(cancelled.recv().await, Some(()));
}
