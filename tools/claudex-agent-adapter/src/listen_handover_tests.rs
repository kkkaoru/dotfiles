use super::*;
use axum::serve::Listener;
use std::time::Duration;

#[test]
fn ephemeral_bind_addr_stays_on_loopback() {
    let v4 = ephemeral_bind_addr("127.0.0.1:8318".parse().unwrap());
    assert!(v4.ip().is_loopback());
    assert_eq!(v4.port(), 0);
    let v6 = ephemeral_bind_addr("[::1]:8318".parse().unwrap());
    assert!(v6.ip().is_loopback());
    assert_eq!(v6.port(), 0);
}

#[test]
fn rebind_state_path_uses_the_canonical_listen_token() {
    let path = rebind_state_path(
        std::path::Path::new("/tmp/claudex"),
        &"127.0.0.1:8318".parse().unwrap(),
    );
    assert!(path.ends_with("rebind.127_0_0_1_8318.json"));
}

#[tokio::test]
async fn ephemeral_rebind_releases_the_canonical_port() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("canonical listener");
    let canonical = listener.local_addr().expect("canonical address");
    let cache = tempfile::tempdir().expect("rebind cache");
    let (handover, rx) = ListenHandover::new(canonical, cache.path().to_path_buf());
    let mut handover_listener = HandoverListener::new(listener, &handover, rx);
    handover.request_ephemeral();
    tokio::time::timeout(Duration::from_secs(2), handover_listener.accept())
        .await
        .ok();
    assert_ne!(handover.advertised_addr(), canonical);
    assert!(handover.advertised_addr().ip().is_loopback());
    TcpListener::bind(canonical)
        .await
        .expect("canonical port must be free after rebind");
    let state = serde_json::from_slice::<RebindState>(
        &std::fs::read(rebind_state_path(cache.path(), &canonical)).expect("rebind state"),
    )
    .expect("decode rebind state");
    assert_eq!(state.listen, handover.advertised_addr());
}
