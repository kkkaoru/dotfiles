use super::super::retained_proxy::{
    RetainedProxy, is_hop_by_hop_header, proxy_http_client, proxy_request,
};
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

async fn hold_accepted_connections(silent: TcpListener) {
    while let Ok((stream, _)) = silent.accept().await {
        // Hold the socket open until the client times out.
        tokio::spawn(async move {
            let _ = stream;
            tokio::time::sleep(Duration::from_secs(2)).await;
        });
    }
}

fn retained(path: &std::path::Path, listen: &str, sessions: &[&str]) -> RetainedProxy {
    retained_with_agents(path, listen, sessions, &[])
}

fn retained_with_agents(
    path: &std::path::Path,
    listen: &str,
    sessions: &[&str],
    agents: &[&str],
) -> RetainedProxy {
    RetainedProxy::from_path(
        path.to_path_buf(),
        RetainedGeneration {
            listen: listen.parse().unwrap(),
            pid: 1,
            build_id: "old".to_owned(),
            session_ids: sessions.iter().map(|id| (*id).to_owned()).collect(),
            agent_ids: agents.iter().map(|id| (*id).to_owned()).collect(),
            agent_ages: std::collections::BTreeMap::new(),
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
        proxy.listen_for_test(),
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
fn retained_proxy_refresh_drops_sticky_when_snapshot_vanishes() {
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
        !proxy.owns("session-a"),
        "missing retained snapshot must drop sticky ownership so live can serve locally"
    );
}

#[test]
fn retained_proxy_refresh_drops_sticky_when_snapshot_is_invalid() {
    let root = tempfile::tempdir().expect("invalid retained fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:9","pid":1,"build_id":"old","session_ids":["session-a"]}"#,
    )
    .expect("write retained");
    let proxy = retained(&path, "127.0.0.1:9", &["session-a"]);
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:9","pid":0,"build_id":"cleared","session_ids":[]}"#,
    )
    .expect("write invalid retained");
    assert!(
        !proxy.owns("session-a"),
        "invalid retained snapshot (pid 0) must drop sticky ownership"
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
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .map(|value| value.as_bytes()),
        Some(b"1".as_slice())
    );
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("connect-failure body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(json["error"]["type"], "invalid_request_error");
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
    handover.request_ephemeral();
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
async fn rebind_listener_times_out_when_advertised_listen_does_not_change() {
    let cache = tempfile::tempdir().expect("rebind timeout cache");
    let (handover, _rx) =
        ListenHandover::new("127.0.0.1:0".parse().unwrap(), cache.path().to_path_buf());
    let started = std::time::Instant::now();
    let response = rebind_listener(
        State(handover.clone()),
        Json(RebindRequest {
            ephemeral: true,
            listen: None,
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        started.elapsed() >= super::helpers::REBIND_POLL_DEADLINE * 4 / 5,
        "timeout path must wait for the rebind deadline"
    );
    assert_eq!(
        handover.advertised_addr(),
        "127.0.0.1:0".parse().unwrap(),
        "failed rebind must keep the original advertised listen"
    );
}

#[tokio::test]
async fn proxy_middleware_forwards_owned_sessions_and_passes_through_others() {
    let upstream = serve_retained_generation(b"from-previous", &["session-a"]).await;
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
        circuit: Default::default(),
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
async fn proxy_middleware_serves_locally_when_retained_is_unreachable() {
    // Post-reboot failure: sticky session_ids still pointed at a retained listen
    // that health-probed as dead, so SubAgent /v1/messages looped on 502.
    let root = tempfile::tempdir().expect("dead retained fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:1","pid":1,"build_id":"old","session_ids":["session-a"]}"#,
    )
    .expect("write retained");
    let cache = tempfile::tempdir().expect("advertised cache");
    let (advertised, _rx) = ListenHandover::new(
        "127.0.0.1:8318".parse().unwrap(),
        cache.path().to_path_buf(),
    );
    let state = Some(HandoverState {
        retained: Some(Arc::new(retained(&path, "127.0.0.1:1", &["session-a"]))),
        advertised: Some(advertised),
        client: proxy_http_client(),
        circuit: Default::default(),
    });
    let app = Router::new()
        .route("/v1/messages", post(|| async { "live-local" }))
        .layer(middleware::from_fn_with_state(
            state,
            proxy_retained_sessions,
        ));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("dead retained listener");
    let addr = listener.local_addr().expect("dead retained address");
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
        .expect("sticky after dead retained");
    assert_eq!(
        response.text().await.expect("live body"),
        "live-local",
        "unreachable retained must fall through to the live generation"
    );
    assert!(
        !path.exists(),
        "dead retained snapshot must be cleared after the probe"
    );
}

#[tokio::test]
async fn proxy_middleware_keeps_retained_when_health_probe_times_out() {
    // Accept connections but never answer /health. A single slow sample must
    // fall through to live without terminating the retained generation.
    let silent = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("silent retained listener");
    let silent_addr = silent.local_addr().expect("silent retained address");
    tokio::spawn(hold_accepted_connections(silent));
    let root = tempfile::tempdir().expect("slow retained fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listen":"{silent_addr}","pid":1,"build_id":"old","session_ids":["session-a"]}}"#
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
            &silent_addr.to_string(),
            &["session-a"],
        ))),
        advertised: Some(advertised),
        client: proxy_http_client(),
        circuit: Default::default(),
    });
    let app = Router::new()
        .route("/v1/messages", post(|| async { "live-after-slow-health" }))
        .layer(middleware::from_fn_with_state(
            state,
            proxy_retained_sessions,
        ));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("slow retained proxy listener");
    let addr = listener.local_addr().expect("slow retained proxy address");
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
        .expect("sticky after slow retained health");
    assert_eq!(
        response.text().await.expect("live body"),
        "live-after-slow-health",
        "timed-out retained health must fall through to live"
    );
    assert!(
        path.exists(),
        "timed-out /health must not clear the retained snapshot"
    );
}

#[tokio::test]
async fn proxy_middleware_serves_locally_when_retained_session_is_idle() {
    let upstream = serve_idle_retained_generation().await;
    let root = tempfile::tempdir().expect("idle retained fixture");
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
        circuit: Default::default(),
    });
    let app = Router::new()
        .route("/v1/messages", post(|| async { "live-after-idle" }))
        .layer(middleware::from_fn_with_state(
            state,
            proxy_retained_sessions,
        ));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("idle retained listener");
    let addr = listener.local_addr().expect("idle retained address");
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
        .expect("sticky after idle retained");
    assert_eq!(
        response.text().await.expect("live body"),
        "live-after-idle",
        "idle retained generations must not keep sticky sessions forever"
    );
    assert!(!path.exists(), "idle retained snapshot must be cleared");
}

#[tokio::test]
async fn proxy_middleware_serves_locally_when_busy_list_is_stale_without_work() {
    // Retained /health can still list a session in busy_claude_session_ids after
    // the turn drained (active_* counters are zero). Sticky must not keep
    // proxying that quiet generation.
    let upstream = serve_stale_busy_retained_generation(&["session-a"]).await;
    let root = tempfile::tempdir().expect("stale busy fixture");
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
        circuit: Default::default(),
    });
    let app = Router::new()
        .route("/v1/messages", post(|| async { "live-after-stale-busy" }))
        .layer(middleware::from_fn_with_state(
            state,
            proxy_retained_sessions,
        ));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("stale busy listener");
    let addr = listener.local_addr().expect("stale busy address");
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
        .expect("sticky after stale busy");
    assert_eq!(
        response.text().await.expect("live body"),
        "live-after-stale-busy",
        "stale busy_claude_session_ids without active work must fall through"
    );
    assert!(
        !path.exists(),
        "stale busy retained snapshot must be cleared"
    );
}

#[tokio::test]
async fn should_proxy_forgets_one_session_while_retained_is_busy_elsewhere() {
    let upstream = serve_retained_generation(b"from-other", &["session-other"]).await;
    let root = tempfile::tempdir().expect("forget one fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listen":"{upstream}","pid":1,"build_id":"old","session_ids":["session-a","session-b"]}}"#
        ),
    )
    .expect("write retained");
    let proxy = retained(&path, &upstream.to_string(), &["session-a", "session-b"]);
    let sticky = proxy.should_proxy_session("session-a", None).await;
    assert!(
        !sticky,
        "session absent from live busy list must not stay sticky"
    );
    assert!(!proxy.owns("session-a"));
    assert!(
        proxy.owns("session-b"),
        "forgetting one sticky session must keep sibling ownership"
    );
}

#[tokio::test]
async fn should_proxy_keeps_empty_retained_when_last_session_is_forgotten() {
    let upstream = serve_retained_generation(b"from-other", &["session-other"]).await;
    let root = tempfile::tempdir().expect("forget last fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listen":"{upstream}","pid":1,"build_id":"old","session_ids":["session-a"]}}"#
        ),
    )
    .expect("write retained");
    let proxy = retained(&path, &upstream.to_string(), &["session-a"]);
    let sticky = proxy.should_proxy_session("session-a", None).await;
    assert!(!sticky);
    assert!(!proxy.owns("session-a"));
    let kept = crate::launcher::read_retained(&path)
        .expect("read")
        .expect("empty snapshot must remain while retained reports active work");
    assert!(kept.session_ids.is_empty());
    assert_eq!(kept.pid, 1);
}

#[tokio::test]
async fn should_proxy_clears_when_retained_health_status_is_not_ok() {
    let upstream = serve_unhealthy_retained_generation().await;
    let root = tempfile::tempdir().expect("unhealthy retained fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listen":"{upstream}","pid":1,"build_id":"old","session_ids":["session-a"]}}"#
        ),
    )
    .expect("write retained");
    let proxy = retained(&path, &upstream.to_string(), &["session-a"]);
    let sticky = proxy.should_proxy_session("session-a", None).await;
    assert!(!sticky);
    assert!(!proxy.owns("session-a"));
    assert!(
        !path.exists(),
        "unhealthy retained snapshot must be cleared"
    );
}

#[tokio::test]
async fn should_proxy_uses_active_session_list_when_busy_list_is_empty() {
    let upstream = serve_active_only_retained_generation(&["session-a"]).await;
    let root = tempfile::tempdir().expect("active-only fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listen":"{upstream}","pid":1,"build_id":"old","session_ids":["session-a"]}}"#
        ),
    )
    .expect("write retained");
    let proxy = retained(&path, &upstream.to_string(), &["session-a"]);
    let sticky = proxy.should_proxy_session("session-a", None).await;
    assert!(
        sticky,
        "active_claude_session_ids must keep sticky when busy list is empty"
    );
}

#[tokio::test]
async fn new_subagent_in_retained_parent_session_runs_live_without_unsticking_old_subagent() {
    let upstream =
        serve_retained_generation_with_agents(b"from-retained", &["parent"], &["agent-old"]).await;
    let root = tempfile::tempdir().expect("agent sticky fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(r#"{{"listen":"{upstream}","pid":1,"build_id":"old","session_ids":["parent"]}}"#),
    )
    .expect("write retained");
    let proxy = retained(&path, &upstream.to_string(), &["parent"]);
    let sticky_old = proxy
        .should_proxy_session("parent", Some("agent-old"))
        .await;
    assert!(sticky_old, "in-flight retained SubAgent must stay sticky");
    let sticky_new = proxy
        .should_proxy_session("parent", Some("agent-new"))
        .await;
    assert!(
        !sticky_new,
        "newly launched SubAgent id must run on the live binary"
    );
    assert!(
        proxy.owns("parent"),
        "rejecting a new agent id must not forget the parent session"
    );
    let sticky_old_again = proxy
        .should_proxy_session("parent", Some("agent-old"))
        .await;
    assert!(
        sticky_old_again,
        "old SubAgent must remain sticky after a new peer launched"
    );
}

#[tokio::test]
async fn should_proxy_falls_back_to_session_scope_when_agent_ids_field_absent() {
    let upstream = serve_retained_generation(b"from-retained", &["parent"]).await;
    let root = tempfile::tempdir().expect("legacy retained fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(r#"{{"listen":"{upstream}","pid":1,"build_id":"old","session_ids":["parent"]}}"#),
    )
    .expect("write retained");
    let proxy = retained(&path, &upstream.to_string(), &["parent"]);
    let sticky_new = proxy
        .should_proxy_session("parent", Some("agent-new"))
        .await;
    assert!(
        sticky_new,
        "pre-agent-id retained health must keep conservative session sticky"
    );
}

#[tokio::test]
async fn should_proxy_keeps_recent_agent_when_active_ids_drain() {
    let upstream =
        serve_retained_generation_with_agent_field(b"from-retained", &["parent"], Some(&[])).await;
    let root = tempfile::tempdir().expect("drained agents fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(r#"{{"listen":"{upstream}","pid":1,"build_id":"old","session_ids":["parent"]}}"#),
    )
    .expect("write retained");
    let proxy = retained(&path, &upstream.to_string(), &["parent"]);
    proxy.remember_agent_for_test("agent-old");
    let sticky_old = proxy
        .should_proxy_session("parent", Some("agent-old"))
        .await;
    assert!(
        sticky_old,
        "recently-seen SubAgent must stay sticky while active_subagent_agent_ids is empty"
    );
    let sticky_new = proxy
        .should_proxy_session("parent", Some("agent-new"))
        .await;
    assert!(
        !sticky_new,
        "unknown SubAgent id must still run on the live binary"
    );
    assert!(
        proxy.owns("parent"),
        "rejecting a new agent id must not forget the parent session"
    );
}

#[tokio::test]
async fn promote_snapshot_agent_ids_seed_sticky_before_health_refresh() {
    let upstream =
        serve_retained_generation_with_agent_field(b"from-retained", &["parent"], Some(&[])).await;
    let root = tempfile::tempdir().expect("promote seed fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listen":"{upstream}","pid":1,"build_id":"old","session_ids":["parent"],"agent_ids":["agent-seeded"]}}"#
        ),
    )
    .expect("write retained");
    let proxy = retained_with_agents(&path, &upstream.to_string(), &["parent"], &["agent-seeded"]);
    let sticky_seeded = proxy
        .should_proxy_session("parent", Some("agent-seeded"))
        .await;
    assert!(
        sticky_seeded,
        "promote-time agent_ids must seed recent_agents across empty active_subagent_agent_ids"
    );
    let sticky_new = proxy
        .should_proxy_session("parent", Some("agent-new"))
        .await;
    assert!(
        !sticky_new,
        "unknown SubAgent id must still run on the live binary after promote seed"
    );
}

#[tokio::test]
async fn should_proxy_keeps_session_within_idle_grace_after_work() {
    let upstream = serve_stale_busy_retained_generation(&["session-a"]).await;
    let root = tempfile::tempdir().expect("idle grace fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listen":"{upstream}","pid":1,"build_id":"old","session_ids":["session-a"]}}"#
        ),
    )
    .expect("write retained");
    let proxy = retained(&path, &upstream.to_string(), &["session-a"]);
    proxy.mark_recent_work_for_test();
    let sticky = proxy.should_proxy_session("session-a", None).await;
    assert!(
        sticky,
        "brief idle after live work must keep sticky ownership"
    );
    assert!(proxy.owns("session-a"));
    assert!(path.exists(), "grace window must not terminate retained");
}

#[tokio::test]
async fn should_proxy_rejects_sessions_that_are_not_tracked() {
    let root = tempfile::tempdir().expect("untracked sticky fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:9","pid":1,"build_id":"old","session_ids":["session-a"]}"#,
    )
    .expect("write retained");
    let proxy = retained(&path, "127.0.0.1:9", &["session-a"]);
    assert!(
        !proxy.should_proxy_session("session-other", None).await,
        "untracked Claude sessions must never sticky-proxy"
    );
}

#[tokio::test]
async fn should_proxy_clears_when_retained_pid_mismatches() {
    let upstream = serve_retained_generation(b"from-other", &["session-a"]).await;
    let root = tempfile::tempdir().expect("pid mismatch fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listen":"{upstream}","pid":42,"build_id":"old","session_ids":["session-a"]}}"#
        ),
    )
    .expect("write retained");
    // Snapshot pid differs from /health pid (1) so sticky must drop ownership.
    let proxy = RetainedProxy::from_path(
        path.clone(),
        RetainedGeneration {
            listen: upstream,
            pid: 42,
            build_id: "old".to_owned(),
            session_ids: vec!["session-a".to_owned()],
            agent_ids: Vec::new(),
            agent_ages: std::collections::BTreeMap::new(),
        },
    );
    assert!(!proxy.should_proxy_session("session-a", None).await);
    assert!(!proxy.owns("session-a"));
    assert!(
        !path.exists(),
        "pid mismatch must clear the retained snapshot"
    );
}

/// Accept connections and stall each one past reqwest's probe timeout, so a
/// health check observes `is_timeout` rather than a connect refusal.
async fn accept_and_stall_connections(listener: TcpListener) {
    while let Ok((mut stream, _)) = listener.accept().await {
        tokio::spawn(async move {
            let mut buf = [0; 1024];
            let _ = stream.read(&mut buf).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        });
    }
}

#[tokio::test]
async fn should_proxy_times_out_health_without_clearing_retained() {
    let silent = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("silent retained listener");
    let silent_addr = silent.local_addr().expect("silent retained address");
    // Read the request then stall so reqwest's 400ms probe timeout fires
    // (is_timeout), rather than a connect refusal.
    tokio::spawn(accept_and_stall_connections(silent));
    let root = tempfile::tempdir().expect("direct slow health fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listen":"{silent_addr}","pid":1,"build_id":"old","session_ids":["session-a"]}}"#
        ),
    )
    .expect("write retained");
    let proxy = retained(&path, &silent_addr.to_string(), &["session-a"]);
    let started = std::time::Instant::now();
    assert!(
        !proxy.should_proxy_session("session-a", None).await,
        "timed-out /health must fall through without sticky"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "health probe must wait for the request timeout"
    );
    assert!(
        path.exists(),
        "timed-out /health must keep the retained snapshot"
    );
    assert!(proxy.owns("session-a"));
}

#[test]
fn retained_proxy_resets_grace_memory_when_pid_changes() {
    let root = tempfile::tempdir().expect("pid change fixture");
    let path = root.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:9","pid":1,"build_id":"old","session_ids":["session-a"],"agent_ids":["agent-old"]}"#,
    )
    .expect("write retained");
    let proxy = retained_with_agents(&path, "127.0.0.1:9", &["session-a"], &["agent-old"]);
    proxy.mark_recent_work_for_test();
    proxy.remember_agent_for_test("agent-local");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:9","pid":2,"build_id":"new","session_ids":["session-b"],"agent_ids":["agent-new"],"agent_ages":{"agent-new":1}}"#,
    )
    .expect("update retained generation");
    assert!(proxy.owns("session-b"));
    assert!(!proxy.owns("session-a"));
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
        circuit: Default::default(),
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
        circuit: Default::default(),
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
async fn diverted_proxy_connect_failure_is_503_with_retry_after() {
    let addr = serve_dead_canonical_divert().await;
    let response = post_session(addr, "session-a").await;
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get("retry-after")
            .map(|value| value.as_bytes()),
        Some(b"1".as_slice())
    );
    let json: serde_json::Value = response.json().await.expect("json");
    assert_eq!(json["error"]["type"], "invalid_request_error");
}

#[tokio::test]
async fn same_session_handover_burst_opens_a_non_retryable_circuit() {
    let addr = serve_dead_canonical_divert().await;
    for _ in 0..2 {
        let response = post_session(addr, "session-storm").await;
        assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    }
    let opened = post_session(addr, "session-storm").await;
    assert_eq!(opened.status(), reqwest::StatusCode::BAD_REQUEST);
    let json: serde_json::Value = opened.json().await.expect("json");
    assert_eq!(json["error"]["type"], "invalid_request_error");
    let sibling = post_session(addr, "session-other").await;
    assert_eq!(sibling.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
}

async fn serve_dead_canonical_divert() -> SocketAddr {
    let cache = Box::leak(Box::new(tempfile::tempdir().expect("dead canonical cache")));
    let (advertised, _rx) =
        ListenHandover::new("127.0.0.1:1".parse().unwrap(), cache.path().to_path_buf());
    advertised.set_advertised_for_test("127.0.0.1:61915".parse().unwrap());
    serve_rebound_proxy(None, advertised, "must-not-run-local").await
}

async fn post_session(addr: SocketAddr, session: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/messages"))
        .header("x-claude-code-session-id", session)
        .body("{}")
        .send()
        .await
        .expect("session post")
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
        circuit: Default::default(),
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
        circuit: Default::default(),
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
    let upstream = serve_retained_generation(b"from-previous", &["session-a"]).await;
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
        circuit: Default::default(),
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
        circuit: Default::default(),
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

async fn serve_retained_generation(
    body: &'static [u8],
    busy_sessions: &'static [&'static str],
) -> SocketAddr {
    // Legacy retained health omitted active_subagent_agent_ids entirely.
    serve_retained_generation_with_agent_field(body, busy_sessions, None).await
}

async fn serve_retained_generation_with_agents(
    body: &'static [u8],
    busy_sessions: &'static [&'static str],
    agent_ids: &'static [&'static str],
) -> SocketAddr {
    serve_retained_generation_with_agent_field(body, busy_sessions, Some(agent_ids)).await
}

async fn serve_retained_generation_with_agent_field(
    body: &'static [u8],
    busy_sessions: &'static [&'static str],
    agent_ids: Option<&'static [&'static str]>,
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("retained upstream listener");
    let listen = listener.local_addr().expect("retained upstream address");
    let sessions = serde_json::to_string(busy_sessions).expect("busy sessions json");
    let agents = agent_ids.map(|ids| serde_json::to_string(ids).expect("agent ids json"));
    tokio::spawn(run_retained_accept_loop(listener, sessions, agents, body));
    listen
}

async fn run_retained_accept_loop(
    listener: TcpListener,
    sessions: String,
    agents: Option<String>,
    body: &'static [u8],
) {
    while let Some(mut stream) = accept_stream(&listener).await {
        respond_retained_request(&mut stream, &sessions, agents.as_deref(), body).await;
    }
}

async fn accept_stream(listener: &TcpListener) -> Option<tokio::net::TcpStream> {
    listener.accept().await.ok().map(|(stream, _)| stream)
}

async fn respond_retained_request(
    stream: &mut tokio::net::TcpStream,
    sessions: &str,
    agents: Option<&str>,
    body: &[u8],
) {
    let mut buf = vec![0; 4096];
    let n = stream.read(&mut buf).await.unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]);
    let (status_line, payload) = retained_response_for(&request, sessions, agents, body);
    write_http_response(stream, status_line, &payload).await;
}

fn retained_response_for(
    request: &str,
    sessions: &str,
    agents: Option<&str>,
    body: &[u8],
) -> (&'static str, Vec<u8>) {
    if request.starts_with("GET /health") {
        let agents_field = agents
            .map(|ids| format!(r#","active_subagent_agent_ids":{ids}"#))
            .unwrap_or_default();
        let payload = format!(
            concat!(
                r#"{{"status":"ok","pid":1,"protocol_version":1,"build_id":"old","#,
                r#""subscription_max_processes":20,"subscription_timeout_minutes":120,"#,
                r#""active_http_requests":1,"active_provider_turns":1,"#,
                r#""busy_claude_session_ids":{sessions},"active_claude_session_ids":{sessions}{agents}}}"#
            ),
            sessions = sessions,
            agents = agents_field
        );
        ("HTTP/1.1 200 OK", payload.into_bytes())
    } else {
        ("HTTP/1.1 200 OK", body.to_vec())
    }
}

async fn write_http_response(
    stream: &mut tokio::net::TcpStream,
    status_line: &str,
    payload: &[u8],
) {
    let header = format!(
        "{status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    let _ = stream.write_all(header.as_bytes()).await;
    let _ = stream.write_all(payload).await;
}

async fn serve_idle_retained_generation() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("idle retained listener");
    let listen = listener.local_addr().expect("idle retained address");
    tokio::spawn(run_idle_retained_accept_loop(listener));
    listen
}

async fn run_idle_retained_accept_loop(listener: TcpListener) {
    while let Some(mut stream) = accept_stream(&listener).await {
        respond_idle_retained_request(&mut stream).await;
    }
}

async fn respond_idle_retained_request(stream: &mut tokio::net::TcpStream) {
    let mut buf = vec![0; 4096];
    let n = stream.read(&mut buf).await.unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]);
    if !request.starts_with("GET /health") {
        let header = "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(header.as_bytes()).await;
        return;
    }
    let payload = concat!(
        r#"{"status":"ok","pid":1,"protocol_version":1,"build_id":"old","#,
        r#""subscription_max_processes":20,"subscription_timeout_minutes":120,"#,
        r#""active_http_requests":0,"active_provider_turns":0,"#,
        r#""busy_claude_session_ids":[],"active_claude_session_ids":[]}"#
    );
    write_http_response(stream, "HTTP/1.1 200 OK", payload.as_bytes()).await;
}

async fn serve_stale_busy_retained_generation(
    busy_sessions: &'static [&'static str],
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("stale busy retained listener");
    let listen = listener.local_addr().expect("stale busy retained address");
    let sessions = serde_json::to_string(busy_sessions).expect("busy sessions json");
    tokio::spawn(run_stale_busy_retained_accept_loop(listener, sessions));
    listen
}

async fn run_stale_busy_retained_accept_loop(listener: TcpListener, sessions: String) {
    while let Some(mut stream) = accept_stream(&listener).await {
        respond_stale_busy_retained_request(&mut stream, &sessions).await;
    }
}

async fn serve_active_only_retained_generation(
    active_sessions: &'static [&'static str],
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("active-only retained listener");
    let listen = listener.local_addr().expect("active-only retained address");
    let sessions = serde_json::to_string(active_sessions).expect("active sessions json");
    tokio::spawn(run_active_only_retained_accept_loop(listener, sessions));
    listen
}

async fn run_active_only_retained_accept_loop(listener: TcpListener, sessions: String) {
    while let Some(mut stream) = accept_stream(&listener).await {
        respond_active_only_retained_request(&mut stream, &sessions).await;
    }
}

async fn respond_active_only_retained_request(stream: &mut tokio::net::TcpStream, sessions: &str) {
    let mut buf = vec![0; 4096];
    let n = stream.read(&mut buf).await.unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]);
    if !request.starts_with("GET /health") {
        let header = "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(header.as_bytes()).await;
        return;
    }
    let payload = format!(
        concat!(
            r#"{{"status":"ok","pid":1,"protocol_version":1,"build_id":"old","#,
            r#""subscription_max_processes":20,"subscription_timeout_minutes":120,"#,
            r#""active_http_requests":1,"active_provider_turns":0,"#,
            r#""busy_claude_session_ids":[],"active_claude_session_ids":{sessions}}}"#
        ),
        sessions = sessions
    );
    write_http_response(stream, "HTTP/1.1 200 OK", payload.as_bytes()).await;
}

async fn serve_unhealthy_retained_generation() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("unhealthy retained listener");
    let listen = listener.local_addr().expect("unhealthy retained address");
    tokio::spawn(run_unhealthy_retained_accept_loop(listener));
    listen
}

async fn run_unhealthy_retained_accept_loop(listener: TcpListener) {
    while let Some(mut stream) = accept_stream(&listener).await {
        respond_unhealthy_retained_request(&mut stream).await;
    }
}

async fn respond_unhealthy_retained_request(stream: &mut tokio::net::TcpStream) {
    let mut buf = vec![0; 4096];
    let n = stream.read(&mut buf).await.unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]);
    if !request.starts_with("GET /health") {
        let header = "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(header.as_bytes()).await;
        return;
    }
    let payload = concat!(
        r#"{"status":"degraded","pid":1,"protocol_version":1,"build_id":"old","#,
        r#""subscription_max_processes":20,"subscription_timeout_minutes":120,"#,
        r#""active_http_requests":1,"active_provider_turns":0,"#,
        r#""busy_claude_session_ids":["session-a"],"active_claude_session_ids":["session-a"]}"#
    );
    write_http_response(stream, "HTTP/1.1 200 OK", payload.as_bytes()).await;
}

async fn respond_stale_busy_retained_request(stream: &mut tokio::net::TcpStream, sessions: &str) {
    let mut buf = vec![0; 4096];
    let n = stream.read(&mut buf).await.unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]);
    if !request.starts_with("GET /health") {
        let header = "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(header.as_bytes()).await;
        return;
    }
    let payload = format!(
        concat!(
            r#"{{"status":"ok","pid":1,"protocol_version":1,"build_id":"old","#,
            r#""subscription_max_processes":20,"subscription_timeout_minutes":120,"#,
            r#""active_http_requests":0,"active_provider_turns":0,"#,
            r#""busy_claude_session_ids":{sessions},"active_claude_session_ids":{sessions}}}"#
        ),
        sessions = sessions
    );
    write_http_response(stream, "HTTP/1.1 200 OK", payload.as_bytes()).await;
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

#[tokio::test]
async fn open_circuit_short_circuits_diverted_proxy_without_another_upstream_call() {
    let cache = tempfile::tempdir().expect("open circuit cache");
    let (advertised, _rx) =
        ListenHandover::new("127.0.0.1:1".parse().unwrap(), cache.path().to_path_buf());
    advertised.set_advertised_for_test("127.0.0.1:61915".parse().unwrap());
    let state = HandoverState {
        retained: None,
        advertised: Some(advertised),
        client: proxy_http_client(),
        circuit: Default::default(),
    };
    for _ in 0..3 {
        state.circuit.note_failure("session-open");
    }
    assert!(super::open_circuit_response(&state, Some("session-open")).is_some());
    let request = Request::builder()
        .uri("/v1/messages")
        .body(Body::empty())
        .expect("request");
    match super::diverted_service_action(&state, Some("session-open"), request).await {
        super::DivertedService::Proxy(response) => {
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        _ => panic!("open circuit must divert to a terminal proxy response"),
    }
    let retry = super::observe_proxy(
        &state,
        None,
        super::handover_circuit::retry_response("retry".to_owned()),
    );
    assert_eq!(retry.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn open_circuit_short_circuits_retained_proxy_when_listen_matches() {
    let cache = tempfile::tempdir().expect("retained circuit cache");
    let listen = "127.0.0.1:8318".parse().unwrap();
    let (handover, _rx) = ListenHandover::new(listen, cache.path().to_path_buf());
    let path = cache.path().join("retained.json");
    std::fs::write(
        &path,
        br#"{"listen":"127.0.0.1:8318","pid":1,"build_id":"old","session_ids":["session-a"]}"#,
    )
    .expect("write retained");
    let circuit = std::sync::Arc::new(super::HandoverCircuit::default());
    for _ in 0..3 {
        circuit.note_failure("session-a");
    }
    let state = Some(HandoverState {
        retained: Some(Arc::new(retained(&path, "127.0.0.1:8318", &["session-a"]))),
        advertised: Some(handover),
        client: proxy_http_client(),
        circuit,
    });
    let app = Router::new()
        .route("/v1/messages", post(|| async { "must-not-run" }))
        .layer(middleware::from_fn_with_state(
            state,
            proxy_retained_sessions,
        ));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("retained circuit listener");
    let addr = listener.local_addr().expect("retained circuit address");
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
        .expect("open circuit retained");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
}
