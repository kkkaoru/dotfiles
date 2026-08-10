use super::*;
use crate::launcher::RetainedGeneration;
use crate::listen_handover::{HandoverListener, ListenHandover};
use axum::{
    Json,
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    serve::Listener,
};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn retained(path: &std::path::Path, listen: &str, sessions: &[&str]) -> RetainedProxy {
    RetainedProxy::from_path(
        path.to_path_buf(),
        RetainedGeneration {
            listen: listen.parse().unwrap(),
            pid: 1,
            build_id: "old".to_owned(),
            session_ids: sessions.iter().map(|id| (*id).to_owned()).collect(),
        },
    )
}

#[test]
fn retained_proxy_owns_only_listed_sessions() {
    let root = tempfile::tempdir().expect("retained proxy fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:9","pid":1,"build_id":"old","session_ids":["session-a"]}"#,
    )
    .expect("write retained");
    let proxy = retained(&path, "127.0.0.1:9", &["session-a"]);
    assert!(proxy.owns("session-a"));
    assert!(!proxy.owns("session-b"));
    assert!(!proxy.owns(""));
}

#[test]
fn retained_proxy_reloads_listen_and_sessions_from_disk() {
    let root = tempfile::tempdir().expect("retained reload fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:9","pid":1,"build_id":"old","session_ids":["session-a"]}"#,
    )
    .expect("write retained");
    let proxy = retained(&path, "127.0.0.1:9", &["session-a"]);
    assert!(proxy.owns("session-a"));
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:65108","pid":1,"build_id":"old","session_ids":["session-busy"]}"#,
    )
    .expect("update retained");
    assert!(!proxy.owns("session-a"));
    assert!(proxy.owns("session-busy"));
    assert_eq!(
        *proxy.listen.read().expect("listen"),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 65108)
    );
}

#[test]
fn rebind_request_requires_ephemeral_or_listen() {
    let ephemeral: RebindRequest = serde_json::from_str(r#"{"ephemeral":true}"#).unwrap();
    assert!(ephemeral.ephemeral);
    assert!(ephemeral.listen.is_none());
    let bind: RebindRequest = serde_json::from_str(r#"{"listen":"127.0.0.1:8318"}"#).unwrap();
    assert!(!bind.ephemeral);
    assert_eq!(bind.listen.as_deref(), Some("127.0.0.1:8318"));
}

#[test]
fn retained_proxy_refresh_keeps_memory_when_snapshot_vanishes() {
    let root = tempfile::tempdir().expect("vanished retained fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:9","pid":1,"build_id":"old","session_ids":["session-a"]}"#,
    )
    .expect("write retained");
    let proxy = retained(&path, "127.0.0.1:9", &["session-a"]);
    std::fs::remove_file(&path).expect("remove retained");
    assert!(
        proxy.owns("session-a"),
        "unreadable snapshot must not drop in-memory retained sessions"
    );
}

#[test]
fn layer_without_handover_has_no_admin_state() {
    let (state, _router) = layer(None);
    assert!(state.is_none());
}

#[test]
fn layer_with_handover_exposes_admin_state_without_retained_env() {
    let cache = tempfile::tempdir().expect("handover cache");
    let (handover, _rx) = ListenHandover::new(
        "127.0.0.1:8318".parse().unwrap(),
        cache.path().to_path_buf(),
    );
    let (state, _router) = layer(Some(handover));
    let state = state.expect("handover state");
    assert!(state.retained.is_none());
}

#[tokio::test]
async fn retained_proxy_forwards_requests_to_the_previous_listener() {
    let listen = serve_http_once(b"from-previous").await;
    let root = tempfile::tempdir().expect("proxy fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(r#"{{"listen":"{listen}","pid":1,"build_id":"old","session_ids":["session-a"]}}"#),
    )
    .expect("write retained");
    let proxy = retained(&path, &listen.to_string(), &["session-a"]);
    let request = Request::builder()
        .uri("/v1/messages?stream=true")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .expect("proxy request");
    let response = proxy.proxy(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("proxy body");
    assert_eq!(&body[..], b"from-previous");
}

#[tokio::test]
async fn retained_proxy_reports_upstream_connect_failure() {
    let root = tempfile::tempdir().expect("proxy failure fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:1","pid":1,"build_id":"old","session_ids":["session-a"]}"#,
    )
    .expect("write retained");
    let proxy = retained(&path, "127.0.0.1:1", &["session-a"]);
    let request = Request::builder()
        .uri("/v1/messages")
        .method("POST")
        .body(Body::empty())
        .expect("proxy request");
    let response = proxy.proxy(request).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn rebind_listener_rejects_invalid_and_empty_requests() {
    let cache = tempfile::tempdir().expect("rebind cache");
    let (handover, _rx) =
        ListenHandover::new("127.0.0.1:0".parse().unwrap(), cache.path().to_path_buf());
    let invalid = rebind_listener(
        State(handover.clone()),
        Json(RebindRequest {
            ephemeral: false,
            listen: Some("not-a-listen".to_owned()),
        }),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let missing = rebind_listener(
        State(handover),
        Json(RebindRequest {
            ephemeral: false,
            listen: None,
        }),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn rebind_listener_returns_the_ephemeral_listen() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("canonical listener");
    let canonical = listener.local_addr().expect("canonical address");
    let cache = tempfile::tempdir().expect("rebind cache");
    let (handover, rx) = ListenHandover::new(canonical, cache.path().to_path_buf());
    let mut handover_listener = HandoverListener::new(listener, &handover, rx);
    let driver = tokio::spawn(async move {
        let _ = tokio::time::timeout(Duration::from_secs(2), handover_listener.accept()).await;
    });
    let response = rebind_listener(
        State(handover.clone()),
        Json(RebindRequest {
            ephemeral: true,
            listen: None,
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_ne!(handover.advertised_addr(), canonical);
    driver.abort();
}

async fn serve_http_once(body: &'static [u8]) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("upstream listener");
    let listen = listener.local_addr().expect("upstream address");
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut buf = vec![0; 2048];
        let _ = stream.read(&mut buf).await;
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(header.as_bytes()).await;
        let _ = stream.write_all(body).await;
    });
    listen
}
