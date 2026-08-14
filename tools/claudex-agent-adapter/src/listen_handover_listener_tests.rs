use super::*;
use axum::serve::Listener;
use std::time::Duration;
use tokio::net::TcpListener;

#[tokio::test]
async fn applying_none_keeps_the_existing_listener() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let canonical = listener.local_addr().expect("canonical address");
    let cache = tempfile::tempdir().expect("cache");
    let (handover, request) = ListenHandover::new(canonical, cache.path().to_path_buf());
    let mut listener = HandoverListener::new(listener, &handover, request);

    listener
        .apply(HandoverCommand::None)
        .await
        .expect("none is a no-op");
    assert_eq!(
        listener.local_addr().expect("advertised address"),
        canonical
    );
}

#[tokio::test]
async fn bind_failure_keeps_the_current_listener() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let canonical = listener.local_addr().expect("canonical address");
    let cache = tempfile::tempdir().expect("cache");
    let (handover, request) = ListenHandover::new(canonical, cache.path().to_path_buf());
    let mut listener = HandoverListener::new(listener, &handover, request);

    let error = listener
        .apply(HandoverCommand::Bind(canonical))
        .await
        .expect_err("binding the occupied canonical address must fail");
    assert!(error.to_string().contains("bind listener during handover"));
    assert_eq!(
        listener.local_addr().expect("advertised address"),
        canonical
    );
}

#[tokio::test]
async fn closed_handover_request_is_ignored() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let canonical = listener.local_addr().expect("canonical address");
    let cache = tempfile::tempdir().expect("cache");
    let (handover, request) = ListenHandover::new(canonical, cache.path().to_path_buf());
    let mut listener = HandoverListener::new(listener, &handover, request);
    drop(handover);

    let changed = listener.request.changed().await;
    assert!(changed.is_err());
    listener.apply_changed_request(changed).await;
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert_eq!(
        listener.local_addr().expect("advertised address"),
        canonical
    );
}
