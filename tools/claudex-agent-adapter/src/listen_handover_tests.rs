use super::*;
use axum::serve::Listener;
use std::{net::SocketAddr, time::Duration};
use tokio::net::{TcpListener, TcpStream};

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
fn canonical_addr_stays_fixed_after_advertised_rebind() {
    let canonical = "127.0.0.1:8318".parse().unwrap();
    let cache = tempfile::tempdir().expect("canonical cache");
    let (handover, _rx) = ListenHandover::new(canonical, cache.path().to_path_buf());
    assert_eq!(handover.canonical_addr(), canonical);
    assert_eq!(handover.advertised_addr(), canonical);
    handover.set_advertised_for_test("127.0.0.1:60104".parse().unwrap());
    assert_eq!(handover.canonical_addr(), canonical);
    assert_eq!(handover.service_addr(), canonical);
    assert_eq!(
        handover.advertised_addr(),
        "127.0.0.1:60104".parse().unwrap()
    );
}

#[test]
fn parse_service_listen_keeps_client_port_off_warm_start_ephemeral() {
    let warm = "127.0.0.1:62486".parse().unwrap();
    let service = "127.0.0.1:8318".parse().unwrap();
    assert_eq!(parse_service_listen(Some("127.0.0.1:8318"), warm), service);
    assert_eq!(parse_service_listen(None, warm), warm);
    assert_eq!(parse_service_listen(Some("not-a-socket"), warm), warm);
}

#[test]
fn from_runtime_bind_reads_service_listen_env() {
    let bind: SocketAddr = "127.0.0.1:62486".parse().unwrap();
    let service: SocketAddr = "127.0.0.1:8318".parse().unwrap();
    let cache = tempfile::tempdir().expect("runtime bind cache");
    let previous = std::env::var_os(SERVICE_LISTEN_ENV);
    unsafe { std::env::set_var(SERVICE_LISTEN_ENV, service.to_string()) };
    let (handover, _rx) = ListenHandover::from_runtime_bind(bind, cache.path().to_path_buf());
    assert_eq!(handover.advertised_addr(), bind);
    assert_eq!(handover.service_addr(), service);
    match previous {
        Some(value) => unsafe { std::env::set_var(SERVICE_LISTEN_ENV, value) },
        None => unsafe { std::env::remove_var(SERVICE_LISTEN_ENV) },
    }

    let cache = tempfile::tempdir().expect("runtime bind fallback");
    let previous = std::env::var_os(SERVICE_LISTEN_ENV);
    unsafe { std::env::remove_var(SERVICE_LISTEN_ENV) };
    let (handover, _rx) = ListenHandover::from_runtime_bind(bind, cache.path().to_path_buf());
    assert_eq!(handover.service_addr(), bind);
    if let Some(value) = previous {
        unsafe { std::env::set_var(SERVICE_LISTEN_ENV, value) };
    }
}

#[test]
fn service_addr_stays_on_client_port_after_warm_start_promote() {
    let warm = "127.0.0.1:62486".parse().unwrap();
    let service = "127.0.0.1:8318".parse().unwrap();
    let cache = tempfile::tempdir().expect("service cache");
    let (handover, _rx) =
        ListenHandover::new_with_service(warm, service, cache.path().to_path_buf());
    assert_eq!(handover.canonical_addr(), warm);
    assert_eq!(handover.service_addr(), service);
    handover.set_advertised_for_test(service);
    assert_eq!(handover.advertised_addr(), service);
    assert_eq!(handover.service_addr(), service);
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

#[tokio::test]
async fn handover_listener_accepts_a_tcp_connection() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("canonical listener");
    let canonical = listener.local_addr().expect("canonical address");
    let cache = tempfile::tempdir().expect("accept cache");
    let (handover, rx) = ListenHandover::new(canonical, cache.path().to_path_buf());
    let mut handover_listener = HandoverListener::new(listener, &handover, rx);
    assert_eq!(
        Listener::local_addr(&handover_listener).expect("advertised addr"),
        canonical
    );
    let connect = tokio::spawn(async move { TcpStream::connect(canonical).await });
    let (_stream, _peer) = tokio::time::timeout(Duration::from_secs(2), handover_listener.accept())
        .await
        .expect("accept timeout");
    connect.await.expect("join connector").expect("connected");
}

#[tokio::test]
async fn bind_rebind_moves_to_the_requested_listen() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("canonical listener");
    let canonical = listener.local_addr().expect("canonical address");
    let reserved = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("reserve target");
    let target = reserved.local_addr().expect("target address");
    drop(reserved);
    let cache = tempfile::tempdir().expect("bind cache");
    let (handover, rx) = ListenHandover::new(canonical, cache.path().to_path_buf());
    let mut handover_listener = HandoverListener::new(listener, &handover, rx);
    handover.request_bind(target);
    tokio::time::timeout(Duration::from_secs(2), handover_listener.accept())
        .await
        .ok();
    assert_eq!(handover.advertised_addr(), target);
    assert_eq!(handover.canonical_addr(), canonical);
}

#[tokio::test]
async fn failed_rebind_keeps_accepting_on_canonical() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("canonical listener");
    let canonical = listener.local_addr().expect("canonical address");
    let cache_file = tempfile::NamedTempFile::new().expect("rebind file cache");
    let (handover, rx) = ListenHandover::new(canonical, cache_file.path().to_path_buf());
    let mut handover_listener = HandoverListener::new(listener, &handover, rx);
    handover.request_ephemeral();
    let connect = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        TcpStream::connect(canonical).await
    });
    let accepted = tokio::time::timeout(Duration::from_secs(2), handover_listener.accept()).await;
    assert!(
        accepted.is_ok(),
        "listener must keep accepting after a failed rebind"
    );
    assert_eq!(handover.advertised_addr(), canonical);
    connect.await.expect("join connector").expect("connected");
}
