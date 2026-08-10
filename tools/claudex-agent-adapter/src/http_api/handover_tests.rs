use super::*;
use crate::launcher::RetainedGeneration;
use crate::listen_handover::{HandoverListener, ListenHandover};
use axum::{
    Json, Router,
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware,
    routing::post,
    serve::Listener,
};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
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

#[tokio::test]
async fn proxy_middleware_forwards_owned_sessions_and_passes_through_others() {
    let upstream = serve_http_once(b"from-previous").await;
    let root = tempfile::tempdir().expect("middleware fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listen":"{upstream}","pid":1,"build_id":"old","session_ids":["session-a"]}}"#
        ),
    )
    .expect("write retained");
    let cache = tempfile::tempdir().expect("advertised cache");
    let (advertised, _rx) = ListenHandover::new(
        "127.0.0.1:8318".parse().unwrap(),
        cache.path().to_path_buf(),
    );
    let state = Some(HandoverState {
        retained: Some(Arc::new(retained(
            &path,
            &upstream.to_string(),
            &["session-a"],
        ))),
        advertised: Some(advertised),
        client: proxy_http_client(),
    });
    let app = Router::new()
        .route("/v1/messages", post(|| async { "local" }))
        .layer(middleware::from_fn_with_state(
            state,
            proxy_retained_sessions,
        ));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("middleware listener");
    let addr = listener.local_addr().expect("middleware address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let client = reqwest::Client::new();
    let owned = client
        .post(format!("http://{addr}/v1/messages"))
        .header("x-claude-code-session-id", "session-a")
        .body("{}")
        .send()
        .await
        .expect("owned session");
    assert_eq!(owned.text().await.expect("owned body"), "from-previous");
    let passthrough = client
        .post(format!("http://{addr}/v1/messages"))
        .header("x-claude-code-session-id", "session-b")
        .send()
        .await
        .expect("unowned session");
    assert_eq!(passthrough.text().await.expect("local body"), "local");
    let missing = client
        .post(format!("http://{addr}/v1/messages"))
        .send()
        .await
        .expect("missing session header");
    assert_eq!(missing.text().await.expect("missing body"), "local");
}

#[tokio::test]
async fn rebound_daemon_forwards_idle_keepalive_to_canonical() {
    // Old failure: after live-update rebind, Claude Code keep-alive stayed on
    // the old binary. Cursor SubAgent /v1/messages then never reached :8318.
    let canonical_upstream = serve_http_once(b"from-canonical").await;
    let cache = tempfile::tempdir().expect("rebound cache");
    let (advertised, _rx) = ListenHandover::new(canonical_upstream, cache.path().to_path_buf());
    advertised.set_advertised_for_test("127.0.0.1:61915".parse().unwrap());
    let state = Some(HandoverState {
        retained: None,
        advertised: Some(advertised),
        client: proxy_http_client(),
    });
    let app = Router::new()
        .route("/v1/messages", post(|| async { "old-binary" }))
        .layer(middleware::from_fn_with_state(
            state,
            proxy_retained_sessions,
        ));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("rebound listener");
    let addr = listener.local_addr().expect("rebound address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/messages"))
        .header("x-claude-code-session-id", "5a7a0dcd-idle-tui")
        .body("{}")
        .send()
        .await
        .expect("idle keepalive");
    assert_eq!(
        response.text().await.expect("proxied body"),
        "from-canonical",
        "rebound daemon must send idle Cursor SubAgent launches to the new canonical listener"
    );
}

#[tokio::test]
async fn promoted_warm_start_does_not_proxy_to_dead_ephemeral() {
    // Old failure: after cutover, the new daemon still treated its warm-start
    // port as canonical and proxied :8318 /v1/messages to dead :62486 → 502.
    let cache = tempfile::tempdir().expect("promoted cache");
    let service = "127.0.0.1:8318".parse().unwrap();
    let (advertised, _rx) = ListenHandover::new_with_service(
        "127.0.0.1:62486".parse().unwrap(),
        service,
        cache.path().to_path_buf(),
    );
    advertised.set_advertised_for_test(service);
    let state = Some(HandoverState {
        retained: None,
        advertised: Some(advertised),
        client: proxy_http_client(),
    });
    let app = Router::new()
        .route("/v1/messages", post(|| async { "promoted-primary" }))
        .layer(middleware::from_fn_with_state(
            state,
            proxy_retained_sessions,
        ));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("promoted listener");
    let addr = listener.local_addr().expect("promoted address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/messages"))
        .header("x-claude-code-session-id", "5a7a0dcd-idle-tui")
        .body("{}")
        .send()
        .await
        .expect("promoted primary");
    assert_eq!(
        response.text().await.expect("local primary body"),
        "promoted-primary",
        "promoted daemon must serve locally instead of proxying to its dead warm-start port"
    );
}

#[tokio::test]
async fn rebound_daemon_keeps_in_flight_retained_sessions_local() {
    let canonical_upstream = serve_http_once(b"from-canonical").await;
    let cache = tempfile::tempdir().expect("rebound busy cache");
    let path = cache.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:61915","pid":1,"build_id":"old","session_ids":["busy-session"]}"#,
    )
    .expect("write retained");
    let (advertised, _rx) = ListenHandover::new(canonical_upstream, cache.path().to_path_buf());
    advertised.set_advertised_for_test("127.0.0.1:61915".parse().unwrap());
    let state = Some(HandoverState {
        retained: Some(Arc::new(retained(
            &path,
            "127.0.0.1:61915",
            &["busy-session"],
        ))),
        advertised: Some(advertised),
        client: proxy_http_client(),
    });
    let app = Router::new()
        .route("/v1/messages", post(|| async { "old-inflight" }))
        .layer(middleware::from_fn_with_state(
            state,
            proxy_retained_sessions,
        ));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("rebound busy listener");
    let addr = listener.local_addr().expect("rebound busy address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/messages"))
        .header("x-claude-code-session-id", "busy-session")
        .body("{}")
        .send()
        .await
        .expect("busy session");
    assert_eq!(
        response.text().await.expect("local inflight body"),
        "old-inflight",
        "in-flight retained sessions must stay on the rebound daemon"
    );
}

#[tokio::test]
async fn proxy_middleware_serves_locally_when_retained_listen_is_self() {
    let cache = tempfile::tempdir().expect("self proxy fixture");
    let listen = "127.0.0.1:8318".parse().unwrap();
    let (handover, _rx) = ListenHandover::new(listen, cache.path().to_path_buf());
    let path = cache.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:8318","pid":1,"build_id":"old","session_ids":["session-a"]}"#,
    )
    .expect("write retained");
    let state = Some(HandoverState {
        retained: Some(Arc::new(retained(&path, "127.0.0.1:8318", &["session-a"]))),
        advertised: Some(handover),
        client: proxy_http_client(),
    });
    let app = Router::new()
        .route("/v1/messages", post(|| async { "local" }))
        .layer(middleware::from_fn_with_state(
            state,
            proxy_retained_sessions,
        ));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("self-proxy listener");
    let addr = listener.local_addr().expect("self-proxy address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/messages"))
        .header("x-claude-code-session-id", "session-a")
        .body("{}")
        .send()
        .await
        .expect("self-owned session");
    assert_eq!(
        response.text().await.expect("self-owned body"),
        "local",
        "retained generation must serve locally instead of proxying to itself"
    );
}

#[tokio::test]
async fn rebound_daemon_forwards_requests_without_session_header() {
    let canonical_upstream = serve_http_once(b"from-canonical").await;
    let cache = tempfile::tempdir().expect("rebound anonymous cache");
    let (advertised, _rx) = ListenHandover::new(canonical_upstream, cache.path().to_path_buf());
    advertised.set_advertised_for_test("127.0.0.1:61915".parse().unwrap());
    let addr = serve_rebound_proxy(None, advertised, "old-binary").await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/messages"))
        .body("{}")
        .send()
        .await
        .expect("anonymous keepalive");
    assert_eq!(
        response.text().await.expect("proxied body"),
        "from-canonical"
    );
}

#[tokio::test]
async fn rebound_daemon_forwards_unowned_retained_sessions() {
    let canonical_upstream = serve_http_once(b"from-canonical").await;
    let cache = tempfile::tempdir().expect("rebound unowned cache");
    let path = cache.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:61915","pid":1,"build_id":"old","session_ids":["busy-session"]}"#,
    )
    .expect("write retained");
    let (advertised, _rx) = ListenHandover::new(canonical_upstream, cache.path().to_path_buf());
    advertised.set_advertised_for_test("127.0.0.1:61915".parse().unwrap());
    let addr = serve_rebound_proxy(
        Some(retained(&path, "127.0.0.1:61915", &["busy-session"])),
        advertised,
        "old-binary",
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/messages"))
        .header("x-claude-code-session-id", "idle-other")
        .body("{}")
        .send()
        .await
        .expect("unowned rebound session");
    assert_eq!(
        response.text().await.expect("proxied unowned body"),
        "from-canonical"
    );
}

#[tokio::test]
async fn rebound_daemon_forwards_when_retained_listen_is_not_self() {
    let canonical_upstream = serve_http_once(b"from-canonical").await;
    let cache = tempfile::tempdir().expect("rebound foreign cache");
    let path = cache.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:9","pid":1,"build_id":"old","session_ids":["busy-session"]}"#,
    )
    .expect("write retained");
    let (advertised, _rx) = ListenHandover::new(canonical_upstream, cache.path().to_path_buf());
    advertised.set_advertised_for_test("127.0.0.1:61915".parse().unwrap());
    let addr = serve_rebound_proxy(
        Some(retained(&path, "127.0.0.1:9", &["busy-session"])),
        advertised,
        "old-binary",
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/messages"))
        .header("x-claude-code-session-id", "busy-session")
        .body("{}")
        .send()
        .await
        .expect("foreign retained session");
    assert_eq!(
        response.text().await.expect("proxied foreign body"),
        "from-canonical"
    );
}

#[tokio::test]
async fn proxy_middleware_without_advertised_listen_uses_retained_only() {
    let upstream = serve_http_once(b"from-previous").await;
    let root = tempfile::tempdir().expect("no advertised fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listen":"{upstream}","pid":1,"build_id":"old","session_ids":["session-a"]}}"#
        ),
    )
    .expect("write retained");
    let state = Some(HandoverState {
        retained: Some(Arc::new(retained(
            &path,
            &upstream.to_string(),
            &["session-a"],
        ))),
        advertised: None,
        client: proxy_http_client(),
    });
    let app = Router::new()
        .route("/v1/messages", post(|| async { "local" }))
        .layer(middleware::from_fn_with_state(
            state,
            proxy_retained_sessions,
        ));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("no-advertised listener");
    let addr = listener.local_addr().expect("no-advertised address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let client = reqwest::Client::new();
    let owned = client
        .post(format!("http://{addr}/v1/messages"))
        .header("x-claude-code-session-id", "session-a")
        .body("{}")
        .send()
        .await
        .expect("owned without advertised");
    assert_eq!(owned.text().await.expect("owned body"), "from-previous");
    let local = client
        .post(format!("http://{addr}/v1/messages"))
        .send()
        .await
        .expect("anonymous without advertised");
    assert_eq!(local.text().await.expect("local body"), "local");
}

#[tokio::test]
async fn proxy_request_reports_unreadable_body() {
    let listen = serve_http_once(b"unused").await;
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(1);
    tx.try_send(Err(std::io::Error::other("broken body")))
        .expect("enqueue broken body");
    drop(tx);
    let request = Request::builder()
        .uri("/v1/messages?stream=true")
        .method("POST")
        .header("connection", "keep-alive")
        .header("host", "127.0.0.1")
        .header("transfer-encoding", "chunked")
        .header("x-claude-code-session-id", "session-a")
        .body(Body::from_stream(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
        .expect("broken body request");
    let response = proxy_request(&proxy_http_client(), listen, request).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn proxy_request_forwards_query_and_strips_hop_by_hop_headers() {
    let listen = serve_http_once(b"from-previous").await;
    let request = Request::builder()
        .uri("/v1/messages?stream=true")
        .method("POST")
        .header("content-type", "application/json")
        .header("connection", "keep-alive")
        .header("host", "127.0.0.1")
        .header("transfer-encoding", "chunked")
        .header("upgrade", "websocket")
        .body(Body::from("{}"))
        .expect("hop-by-hop request");
    let response = proxy_request(&proxy_http_client(), listen, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("proxy body");
    assert_eq!(&body[..], b"from-previous");
}

#[tokio::test]
async fn proxy_request_surfaces_truncated_upstream_chunks() {
    let listen = serve_http_then_reset().await;
    let request = Request::builder()
        .uri("/v1/messages")
        .method("POST")
        .body(Body::from("{}"))
        .expect("truncated upstream request");
    let response = proxy_request(&proxy_http_client(), listen, request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(response.into_body(), 1024).await;
}

#[test]
fn hop_by_hop_headers_are_not_forwarded() {
    for name in [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailers",
        "transfer-encoding",
        "upgrade",
        "host",
    ] {
        assert!(
            is_hop_by_hop_header(&axum::http::HeaderName::from_static(name)),
            "{name} must be treated as hop-by-hop"
        );
    }
    assert!(!is_hop_by_hop_header(&axum::http::HeaderName::from_static(
        "x-claude-code-session-id"
    )));
}

#[test]
fn retained_proxy_targets_the_current_listen() {
    let root = tempfile::tempdir().expect("targets fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:52864","pid":1,"build_id":"old","session_ids":["session-a"]}"#,
    )
    .expect("write retained");
    let proxy = retained(&path, "127.0.0.1:52864", &["session-a"]);
    assert!(proxy.targets("127.0.0.1:52864".parse().unwrap()));
    assert!(!proxy.targets("127.0.0.1:8318".parse().unwrap()));
}

async fn serve_rebound_proxy(
    retained: Option<RetainedProxy>,
    advertised: ListenHandover,
    local_body: &'static str,
) -> SocketAddr {
    let local_body = local_body.to_owned();
    let state = Some(HandoverState {
        retained: retained.map(Arc::new),
        advertised: Some(advertised),
        client: proxy_http_client(),
    });
    let app = Router::new()
        .route(
            "/v1/messages",
            post(move || {
                let local_body = local_body.clone();
                async move { local_body }
            }),
        )
        .layer(middleware::from_fn_with_state(
            state,
            proxy_retained_sessions,
        ));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("rebound listener");
    let addr = listener.local_addr().expect("rebound address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    addr
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

async fn serve_http_then_reset() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("reset listener");
    let listen = listener.local_addr().expect("reset address");
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut buf = vec![0; 2048];
        let _ = stream.read(&mut buf).await;
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello")
            .await;
        drop(stream);
    });
    listen
}
