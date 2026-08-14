use super::*;
use crate::ADAPTER_PROTOCOL_VERSION;
use crate::agent_backend::{BackendKind, BackendRoute};
use crate::launcher::health::Health;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::super::{AdapterOptions, LOCAL_TOKEN, ServiceConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn http_response(status_line: &str, body: &str) -> String {
    format!(
        "{status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn http_ok(body: &str) -> String {
    http_response("HTTP/1.1 200 OK", body)
}

async fn accept_and_write(listener: &TcpListener, response: &str) {
    let Ok((mut stream, _)) = listener.accept().await else {
        return;
    };
    let mut request = [0; 4096];
    let _ = stream.read(&mut request).await;
    let _ = stream.write_all(response.as_bytes()).await;
}

async fn wait_until_listen_free(listen: SocketAddr) -> bool {
    loop {
        if listen_is_free(listen) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn serve_http_once(listener: TcpListener, response: String) {
    accept_and_write(&listener, &response).await;
}

fn health(listener_handover: bool, pid: Option<u32>) -> Health {
    Health {
        status: "ok".to_owned(),
        pid,
        protocol_version: ADAPTER_PROTOCOL_VERSION,
        build_id: "old-build".to_owned(),
        model: "opus".to_owned(),
        codex_config_fingerprint: "codex".to_owned(),
        service_config_fingerprint: "service".to_owned(),
        backend_routes: Vec::new(),
        worker_routes: Vec::new(),
        search_worker_routes: Vec::new(),
        subscription_max_processes: 20,
        subscription_timeout_minutes: 120,
        subagent_hard_timeout_seconds: None,
        recovery_generation: None,
        active_http_requests: 1,
        active_provider_turns: 1,
        active_subagent_models: BTreeMap::new(),
        listener_handover,
        listen: Some("127.0.0.1:8318".to_owned()),
        active_claude_session_ids: vec!["session-a".to_owned()],
        busy_claude_session_ids: Vec::new(),
        idle_seconds: None,
        active_subagent_agent_ids: Vec::new(),
        recent_subagent_agent_ids: BTreeMap::new(),
    }
}

#[test]
fn handover_requires_a_capable_daemon_pid() {
    assert!(!handover_supported(&health(false, Some(12))));
    assert!(!handover_supported(&health(true, None)));
    assert!(!handover_supported(&health(true, Some(0))));
    assert!(handover_supported(&health(true, Some(12))));
}

#[test]
fn current_build_readiness_checks_expected_pid() {
    let mut ready = health(true, Some(12));
    ready.build_id = env!("CLAUDEX_BUILD_ID").to_owned();
    assert!(current_build_ready(&ready, None));
    assert!(current_build_ready(&ready, Some(12)));
    assert!(!current_build_ready(&ready, Some(13)));
    ready.pid = None;
    assert!(current_build_ready(&ready, Some(13)));
}

#[cfg(unix)]
#[test]
fn terminate_started_ignores_an_unrelated_pid() {
    let root = tempfile::tempdir().expect("terminate fixture");
    let config = config_at(
        "127.0.0.1:8318".parse().unwrap(),
        root.path(),
        PathBuf::from("/tmp/adapter"),
    );
    terminate_started(u32::MAX, &config);
}

#[test]
fn live_update_requires_the_same_service_fingerprints() {
    let config = ServiceConfig {
        options: AdapterOptions {
            routes: vec![BackendRoute::new("test-model", BackendKind::CodexAppServer)],
            listen: "127.0.0.1:8318".parse().unwrap(),
            model: "test-model".to_owned(),
            subscription_max_processes: 20,
            subscription_timeout_minutes: 120,
            subagent_hard_timeout_seconds: None,
            model_catalog: crate::provider_config::ModelCatalog::default(),
        },
        token: LOCAL_TOKEN.to_owned(),
        codex_config_fingerprint: "codex".to_owned(),
        service_config_fingerprint: "service".to_owned(),
        executable: PathBuf::from("/tmp/claudex-agent-adapter"),
        log_path: PathBuf::from("/tmp/claudex/adapter.log"),
        lock_path: PathBuf::from("/tmp/claudex/adapter.lock"),
    };
    let matching = health(true, Some(12));
    assert!(live_update_eligible(&matching, &config));
    let mut timeout_changed = matching;
    timeout_changed.service_config_fingerprint = "other-service".to_owned();
    assert!(!live_update_eligible(&timeout_changed, &config));
}

#[tokio::test]
async fn try_canonical_skips_missing_or_zero_pids() {
    let config = ServiceConfig {
        options: AdapterOptions {
            routes: vec![BackendRoute::new("test-model", BackendKind::CodexAppServer)],
            listen: "127.0.0.1:8318".parse().unwrap(),
            model: "test-model".to_owned(),
            subscription_max_processes: 20,
            subscription_timeout_minutes: 120,
            subagent_hard_timeout_seconds: None,
            model_catalog: crate::provider_config::ModelCatalog::default(),
        },
        token: LOCAL_TOKEN.to_owned(),
        codex_config_fingerprint: "codex".to_owned(),
        service_config_fingerprint: "service".to_owned(),
        executable: PathBuf::from("/tmp/claudex-agent-adapter"),
        log_path: PathBuf::from("/tmp/claudex/adapter.log"),
        lock_path: PathBuf::from("/tmp/claudex/adapter.lock"),
    };
    let missing = try_canonical(&reqwest::Client::new(), &config, &health(true, None))
        .await
        .expect("missing pid skip");
    assert_eq!(missing, None);
    let zero = try_canonical(&reqwest::Client::new(), &config, &health(true, Some(0)))
        .await
        .expect("zero pid skip");
    assert_eq!(zero, None);
}

#[tokio::test]
async fn try_canonical_skips_legacy_daemons_without_handover() {
    let config = ServiceConfig {
        options: AdapterOptions {
            routes: vec![BackendRoute::new("test-model", BackendKind::CodexAppServer)],
            listen: "127.0.0.1:8318".parse().unwrap(),
            model: "test-model".to_owned(),
            subscription_max_processes: 20,
            subscription_timeout_minutes: 120,
            subagent_hard_timeout_seconds: None,
            model_catalog: crate::provider_config::ModelCatalog::default(),
        },
        token: LOCAL_TOKEN.to_owned(),
        codex_config_fingerprint: "codex".to_owned(),
        service_config_fingerprint: "service".to_owned(),
        executable: PathBuf::from("/tmp/claudex-agent-adapter"),
        log_path: PathBuf::from("/tmp/claudex/adapter.log"),
        lock_path: PathBuf::from("/tmp/claudex/adapter.lock"),
    };
    let url = try_canonical(&reqwest::Client::new(), &config, &health(false, Some(9)))
        .await
        .expect("legacy skip");
    assert_eq!(url, None);
}

#[test]
fn retains_busy_and_co_retained_active_sessions_together() {
    let mut health = health(true, Some(12));
    health.busy_claude_session_ids = vec!["busy-a".to_owned()];
    health.active_claude_session_ids = vec!["busy-a".to_owned(), "idle-tui".to_owned()];
    assert_eq!(
        retained_session_ids(&health),
        ["busy-a".to_owned(), "idle-tui".to_owned()],
        "quiet co-retained sessions must survive cutover with a busy sibling"
    );
}

#[test]
fn retains_all_active_sessions_on_legacy_busy_health() {
    let mut health = health(true, Some(12));
    health.active_http_requests = 1;
    health.busy_claude_session_ids.clear();
    assert_eq!(retained_session_ids(&health), ["session-a"]);
}

#[test]
fn current_build_ready_ignores_fingerprint_and_requires_this_build() {
    let mut health = health(true, Some(12));
    health.build_id = env!("CLAUDEX_BUILD_ID").to_owned();
    health.model = "unrelated-model".to_owned();
    assert!(current_build_ready(&health, Some(12)));
    assert!(!current_build_ready(&health, Some(99)));
    health.pid = None;
    assert!(
        current_build_ready(&health, Some(12)),
        "missing pid must not roll back a current-build listener"
    );
    health.build_id = "old-build".to_owned();
    assert!(!current_build_ready(&health, Some(12)));
    health.build_id = env!("CLAUDEX_BUILD_ID").to_owned();
    health.status = "unavailable".to_owned();
    assert!(!current_build_ready(&health, None));
}

#[test]
fn retains_no_sessions_for_an_idle_tui() {
    let mut health = health(true, Some(12));
    health.active_http_requests = 0;
    health.active_provider_turns = 0;
    health.busy_claude_session_ids.clear();
    health.active_claude_session_ids = vec!["idle-tui".to_owned()];
    assert!(retained_session_ids(&health).is_empty());
}

#[test]
fn retains_recent_sessions_within_sticky_idle_grace() {
    let mut health = health(true, Some(12));
    health.active_http_requests = 0;
    health.active_provider_turns = 0;
    health.busy_claude_session_ids.clear();
    health.active_claude_session_ids = vec!["session-a".to_owned()];
    health.idle_seconds = Some(5);
    assert_eq!(
        retained_session_ids(&health),
        ["session-a"],
        "brief idle between turns must keep sessions for retained cutover"
    );
    health.idle_seconds = Some(600);
    assert_eq!(
        retained_session_ids(&health),
        ["session-a"],
        "idle within adapter session TTL must keep sessions for retained cutover"
    );
    health.idle_seconds = Some(crate::sticky_grace::STICKY_IDLE_GRACE_SECS + 1);
    assert!(
        retained_session_ids(&health).is_empty(),
        "idle past sticky/session TTL must still release"
    );
    health.idle_seconds = None;
    assert!(
        retained_session_ids(&health).is_empty(),
        "legacy health without idle_seconds stays immediate-release"
    );
}

#[test]
fn warm_agent_ids_union_active_and_recent() {
    let mut health = health(true, Some(12));
    health.active_subagent_agent_ids = vec!["agent-live".to_owned(), "".to_owned()];
    health
        .recent_subagent_agent_ids
        .insert("agent-warm".to_owned(), 3);
    health
        .recent_subagent_agent_ids
        .insert("agent-live".to_owned(), 0);
    assert_eq!(
        warm_agent_ids(&health),
        vec!["agent-live".to_owned(), "agent-warm".to_owned()]
    );
}

#[test]
fn advertised_listen_prefers_health_and_falls_back_to_config() {
    let config = ServiceConfig {
        options: AdapterOptions {
            routes: vec![BackendRoute::new("test-model", BackendKind::CodexAppServer)],
            listen: "127.0.0.1:8318".parse().unwrap(),
            model: "test-model".to_owned(),
            subscription_max_processes: 20,
            subscription_timeout_minutes: 120,
            subagent_hard_timeout_seconds: None,
            model_catalog: crate::provider_config::ModelCatalog::default(),
        },
        token: LOCAL_TOKEN.to_owned(),
        codex_config_fingerprint: "codex".to_owned(),
        service_config_fingerprint: "service".to_owned(),
        executable: PathBuf::from("/tmp/claudex-agent-adapter"),
        log_path: PathBuf::from("/tmp/claudex/adapter.log"),
        lock_path: PathBuf::from("/tmp/claudex/adapter.lock"),
    };
    let mut health = health(true, Some(12));
    health.listen = Some("127.0.0.1:9999".to_owned());
    assert_eq!(
        advertised_listen(&config, &health),
        "127.0.0.1:9999".parse().unwrap()
    );
    health.listen = Some("not-a-listen".to_owned());
    assert_eq!(advertised_listen(&config, &health), config.options.listen);
    health.listen = None;
    assert_eq!(advertised_listen(&config, &health), config.options.listen);
}

#[test]
fn release_previous_ignores_non_adapter_pids() {
    let root = tempfile::tempdir().expect("publish fixture");
    let config = config_at(
        "127.0.0.1:8318".parse().unwrap(),
        root.path(),
        PathBuf::from("/tmp/claudex-agent-adapter"),
    );
    release_previous(&config, std::process::id());
    publish_promoted(&config, 99, 12, config.options.listen, 0).expect("publish empty promote");
    publish_promoted(&config, 99, 12, config.options.listen, 2).expect("publish retained promote");
}

#[test]
fn publish_promoted_clears_empty_retained_snapshot() {
    let root = tempfile::tempdir().expect("empty retained promote fixture");
    let listen = "127.0.0.1:8318".parse().unwrap();
    let config = config_at(listen, root.path(), PathBuf::from("/tmp/adapter"));
    let path = live::write_retained(&config, listen, 12, "old", Vec::new()).unwrap();
    assert!(
        path.exists(),
        "precondition: empty retained snapshot exists"
    );
    publish_promoted(&config, 99, 12, listen, 0).expect("publish promoted state");
    assert!(
        !path.exists(),
        "zero-session promote must drop the retained snapshot so sticky cannot probe a released listen"
    );
}

fn config_at(listen: SocketAddr, root: &Path, executable: PathBuf) -> ServiceConfig {
    ServiceConfig {
        options: AdapterOptions {
            routes: vec![BackendRoute::new("test-model", BackendKind::CodexAppServer)],
            listen,
            model: "test-model".to_owned(),
            subscription_max_processes: 20,
            subscription_timeout_minutes: 120,
            subagent_hard_timeout_seconds: None,
            model_catalog: crate::provider_config::ModelCatalog::default(),
        },
        token: LOCAL_TOKEN.to_owned(),
        codex_config_fingerprint: "codex".to_owned(),
        service_config_fingerprint: "service".to_owned(),
        executable,
        log_path: root.join("adapter.log"),
        lock_path: root.join("adapter.lock"),
    }
}

fn health_body(pid: Option<u32>, active: bool) -> String {
    health_body_with_idle(pid, active, None)
}

fn health_body_with_idle(pid: Option<u32>, active: bool, idle_seconds: Option<u64>) -> String {
    serde_json::json!({
        "status": "ok", "pid": pid, "protocol_version": ADAPTER_PROTOCOL_VERSION,
        "build_id": "old-build", "subscription_max_processes": 20,
        "subscription_timeout_minutes": 120, "active_http_requests": active as usize,
        "idle_seconds": idle_seconds,
    })
    .to_string()
}

async fn health_server(
    canonical: String,
    retained: String,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("health server");
    let address = listener.local_addr().expect("health address");
    let task = tokio::spawn(async move {
        for body in [canonical, retained] {
            accept_and_write(&listener, &http_ok(&body)).await;
        }
    });
    (address, task)
}

#[tokio::test]
async fn release_idle_retained_keeps_generation_within_sticky_idle_grace() {
    let root = tempfile::tempdir().expect("retained grace fixture");
    let (server, task) = health_server(
        health_body(Some(999), false),
        health_body_with_idle(Some(90), false, Some(10)),
    )
    .await;
    let config = config_at(server, root.path(), PathBuf::from("/tmp/adapter"));
    let path =
        live::write_retained(&config, server, 90, "old", vec!["grace-session".to_owned()]).unwrap();
    release_idle_retained(&reqwest::Client::new(), &config).await;
    assert!(
        path.exists(),
        "idle_seconds within sticky grace must keep retained"
    );
    task.abort();
}

#[tokio::test]
async fn release_idle_retained_releases_when_sticky_idle_grace_expires() {
    let root = tempfile::tempdir().expect("retained grace expired fixture");
    let past_grace = crate::sticky_grace::STICKY_IDLE_GRACE_SECS + 1;
    let (server, task) = health_server(
        health_body(Some(999), false),
        health_body_with_idle(Some(91), false, Some(past_grace)),
    )
    .await;
    let config = config_at(server, root.path(), PathBuf::from("/tmp/adapter"));
    let path = live::write_retained(
        &config,
        server,
        91,
        "old",
        vec!["expired-session".to_owned()],
    )
    .unwrap();
    release_idle_retained(&reqwest::Client::new(), &config).await;
    assert!(
        !path.exists(),
        "idle_seconds past sticky grace must release retained"
    );
    task.abort();
}

#[tokio::test]
async fn release_idle_retained_keeps_generation_used_by_canonical_listener() {
    let root = tempfile::tempdir().expect("retained release fixture");
    let (server, task) =
        health_server(health_body(Some(77), false), health_body(None, false)).await;
    let config = config_at(server, root.path(), PathBuf::from("/tmp/adapter"));
    let path = live::write_retained(&config, server, 77, "old", Vec::new()).unwrap();
    release_idle_retained(&reqwest::Client::new(), &config).await;
    assert!(path.exists());
    task.abort();
}

#[tokio::test]
async fn release_idle_retained_releases_idle_active_and_unknown_generations() {
    for (retained_pid, sessions, retained_body, expect_removed) in [
        (
            78,
            vec!["busy-session".to_owned()],
            health_body(Some(78), true),
            false,
        ),
        (
            79,
            vec!["idle-session".to_owned()],
            health_body(Some(79), false),
            true,
        ),
        (
            80,
            vec!["gone-session".to_owned()],
            health_body(Some(81), false),
            true,
        ),
        (81, Vec::new(), health_body(Some(81), true), false),
        (82, Vec::new(), health_body(Some(82), false), true),
    ] {
        let root = tempfile::tempdir().expect("retained release fixture");
        let (server, task) = health_server(health_body(Some(999), false), retained_body).await;
        let config = config_at(server, root.path(), PathBuf::from("/tmp/adapter"));
        let path = live::write_retained(&config, server, retained_pid, "old", sessions).unwrap();
        release_idle_retained(&reqwest::Client::new(), &config).await;
        assert_eq!(
            path.exists(),
            !expect_removed,
            "pid={retained_pid} expect_removed={expect_removed}"
        );
        task.abort();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn try_canonical_fails_closed_when_warm_start_never_becomes_ready() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;
    let root = tempfile::tempdir().expect("warm-start fixture");
    let executable = root.path().join("daemon.sh");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("daemon script");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
        .expect("daemon executable");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("canonical listen");
    let listen = listener.local_addr().expect("canonical address");
    drop(listener);
    let config = config_at(listen, root.path(), executable);
    let started = Instant::now();
    let error = try_canonical(&reqwest::Client::new(), &config, &health(true, Some(12)))
        .await
        .expect_err("warm-start must fail closed");
    assert!(
        error.to_string().contains("wait for warm-start"),
        "{error:#}"
    );
    #[cfg(not(coverage_nightly))]
    let fail_fast_limit = WARM_START_TIMEOUT - std::time::Duration::from_secs(1);
    #[cfg(coverage_nightly)]
    let fail_fast_limit = WARM_START_TIMEOUT / 2;
    assert!(
        started.elapsed() < fail_fast_limit,
        "dead warm-start child must fail before the readiness timeout ({:?}, limit {fail_fast_limit:?})",
        started.elapsed()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn try_canonical_reports_retained_state_write_failure() {
    let root = tempfile::tempdir().expect("retained write fixture");
    let dummy = write_current_build_dummy(root.path());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("canonical listen");
    let listen = listener.local_addr().expect("canonical address");
    tokio::spawn(serve_http_once(
        listener,
        http_ok(r#"{"listen":"127.0.0.1:65100"}"#),
    ));
    let config = config_at(listen, root.path(), dummy.clone());
    let _fail = live::FailRetainedWriteAfter::arm(1);
    let error = try_canonical(&reqwest::Client::new(), &config, &health(true, Some(12)))
        .await
        .expect_err("retained state write must fail");
    kill_dummy(&dummy);
    assert!(
        error.to_string().contains("state") || error.to_string().contains("injected"),
        "{error:#}"
    );
}

#[tokio::test]
async fn request_rebind_parses_success_and_skips_unreachable_listeners() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("rebind listener");
    let listen = listener.local_addr().expect("rebind address");
    tokio::spawn(serve_http_once(
        listener,
        http_ok(r#"{"listen":"127.0.0.1:65100"}"#),
    ));
    let root = tempfile::tempdir().expect("rebind fixture");
    let config = config_at(listen, root.path(), PathBuf::from("/tmp/adapter"));
    let rebound = request_ephemeral_rebind(&reqwest::Client::new(), &config)
        .await
        .expect("ephemeral rebind");
    assert_eq!(rebound.expect("rebind listen").listen, "127.0.0.1:65100");
    let missing = request_bind_listen(
        &reqwest::Client::new(),
        &config_at(
            "127.0.0.1:1".parse().unwrap(),
            root.path(),
            PathBuf::from("/tmp/adapter"),
        ),
        "127.0.0.1:8318".parse().unwrap(),
    )
    .await
    .expect("unreachable rebind");
    assert!(missing.is_none());
}

#[tokio::test]
async fn request_rebind_skips_non_success_responses() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("rebind listener");
    let listen = listener.local_addr().expect("rebind address");
    tokio::spawn(serve_http_once(
        listener,
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
            .to_owned(),
    ));
    let root = tempfile::tempdir().expect("rebind fixture");
    let rejected = request_ephemeral_rebind(
        &reqwest::Client::new(),
        &config_at(listen, root.path(), PathBuf::from("/tmp/adapter")),
    )
    .await
    .expect("rejected rebind");
    assert!(rejected.is_none());
}

#[tokio::test]
async fn listen_is_free_matches_whether_the_port_can_be_bound() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("busy listen");
    let listen = listener.local_addr().expect("busy address");
    assert!(!listen_is_free(listen));
    drop(listener);
    // Parallel llvm-cov suites can briefly keep the port busy after drop.
    let became_free = tokio::time::timeout(Duration::from_secs(2), wait_until_listen_free(listen))
        .await
        .unwrap_or(false);
    assert!(
        became_free,
        "port should become free after the listener drops"
    );
}

#[tokio::test]
async fn wait_until_canonical_released_returns_when_the_port_is_free() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("free listen");
    let listen = listener.local_addr().expect("free address");
    drop(listener);
    let root = tempfile::tempdir().expect("released fixture");
    wait_until_canonical_released(&config_at(
        listen,
        root.path(),
        PathBuf::from("/tmp/adapter"),
    ))
    .await
    .expect("free port");
}

#[tokio::test]
async fn wait_until_canonical_released_times_out_while_the_port_is_busy() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("busy listen");
    let listen = listener.local_addr().expect("busy address");
    let root = tempfile::tempdir().expect("busy fixture");
    let error = wait_until_canonical_released(&config_at(
        listen,
        root.path(),
        PathBuf::from("/tmp/adapter"),
    ))
    .await
    .expect_err("busy port must time out");
    assert!(error.to_string().contains("did not release"));
    drop(listener);
}

#[cfg(unix)]
fn write_current_build_dummy(root: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let executable = root.join("claudex-agent-adapter");
    let script = format!(
        r#"#!/usr/bin/python3
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

def listen_addr():
    args = sys.argv
    for i, arg in enumerate(args):
        if arg == "--listen" and i + 1 < len(args):
            return args[i + 1]
    raise SystemExit("missing --listen")

host, port = listen_addr().rsplit(":", 1)
host = host.strip("[]")
BUILD = {build:?}
PROTOCOL = {protocol}

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        return
    def do_GET(self):
        if self.path.split("?", 1)[0] != "/health":
            self.send_error(404)
            return
        body = json.dumps({{
            "status": "ok",
            "pid": __import__("os").getpid(),
            "protocol_version": PROTOCOL,
            "build_id": BUILD,
            "subscription_max_processes": 20,
            "subscription_timeout_minutes": 120,
            "listener_handover": True,
        }}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_POST(self):
        rebind_started = __import__("os").environ.get("CLAUDEX_TEST_REBIND_STARTED")
        rebind_permit = __import__("os").environ.get("CLAUDEX_TEST_REBIND_PERMIT")
        if rebind_started and rebind_permit:
            open(rebind_started, "w").close()
            deadline = __import__("time").monotonic() + 2
            while not __import__("os").path.exists(rebind_permit):
                if __import__("time").monotonic() >= deadline:
                    self.send_error(504)
                    return
                __import__("time").sleep(0.001)
        self.send_error(501)

HTTPServer((host, int(port)), Handler).serve_forever()
"#,
        build = env!("CLAUDEX_BUILD_ID"),
        protocol = ADAPTER_PROTOCOL_VERSION,
    );
    std::fs::write(&executable, script).expect("dummy script");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
        .expect("dummy executable");
    executable
}

#[cfg(unix)]
fn kill_dummy(executable: &Path) {
    let _ = std::process::Command::new("pkill")
        .args(["-f", &executable.to_string_lossy()])
        .status();
}

#[cfg(unix)]
fn serve_reoccupied_canonical(
    held: &std::net::TcpListener,
    stop_server: &std::sync::atomic::AtomicBool,
) {
    while !stop_server.load(std::sync::atomic::Ordering::SeqCst) {
        if reoccupied_listener_is_idle(held) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(unix)]
fn reoccupied_listener_is_idle(held: &std::net::TcpListener) -> bool {
    match held.accept() {
        Ok((mut stream, _)) => {
            let _ = std::io::Write::write_all(
                &mut stream,
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            false
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => true,
        Err(error) => panic!("accept reoccupied canonical listener: {error}"),
    }
}

#[cfg(unix)]
fn wait_for_rebind_request(
    listener: &std::net::TcpListener,
    stop_server: &std::sync::atomic::AtomicBool,
) -> Option<std::net::TcpStream> {
    loop {
        if stop_server.load(std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        match listener.accept() {
            Ok((stream, _)) => return Some(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("accept rebind request: {error}"),
        }
    }
}

#[cfg(unix)]
fn read_complete_rebind_request(stream: &mut std::net::TcpStream) {
    const MAX_REQUEST_BYTES: usize = 64 * 1024;

    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = std::io::Read::read(stream, &mut chunk).expect("read rebind request");
        assert!(
            read > 0,
            "rebind request ended before its body was complete"
        );
        request.extend_from_slice(&chunk[..read]);
        assert!(
            request.len() <= MAX_REQUEST_BYTES,
            "rebind request exceeded fixture limit"
        );
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = std::str::from_utf8(&request[..header_end]).expect("HTTP request headers");
        let content_length = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find_map(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("HTTP content length"))
            })
            .expect("rebind request content length");
        if request.len() >= header_end + 4 + content_length {
            return;
        }
    }
}

#[cfg(unix)]
fn spawn_reoccupying_canonical(
    listener: TcpListener,
    response: String,
    listen: SocketAddr,
    rebind_started: PathBuf,
    rebind_permit: PathBuf,
    stop_server: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let listener = listener.into_std().expect("canonical std listener");
        listener
            .set_nonblocking(true)
            .expect("nonblocking canonical listener");
        let Some(mut stream) = wait_for_rebind_request(&listener, &stop_server) else {
            return;
        };
        // The accepted socket inherits the listener's nonblocking mode on this
        // platform. Read the complete fixture request in blocking mode rather
        // than treating an initial WouldBlock as an empty request.
        stream
            .set_nonblocking(false)
            .expect("blocking rebind request stream");
        // A single read can stop between HTTP packets under llvm-cov. Closing
        // with unread request-body bytes may reset the socket, making reqwest
        // report the initial rebind as `None` before this test reaches the
        // canonical-port reoccupation it is intended to exercise.
        read_complete_rebind_request(&mut stream);
        std::io::Write::write_all(&mut stream, response.as_bytes()).expect("rebind response");
        drop(stream);
        drop(listener);
        // request_bind_listen is called only after wait_until_canonical_released
        // has observed the old canonical listener release.  Waiting for the
        // warm dummy's POST makes reoccupation deterministic instead of racing
        // that observation with an arbitrary sleep.
        while !rebind_started.exists() && !stop_server.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        if stop_server.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let held = std::net::TcpListener::bind(listen).expect("reoccupy canonical port");
        held.set_nonblocking(true)
            .expect("nonblocking reoccupied canonical listener");
        std::fs::write(rebind_permit, "ready").expect("permit warm rebind response");
        serve_reoccupied_canonical(&held, &stop_server);
        drop(held);
    })
}

#[cfg(unix)]
#[tokio::test]
async fn try_canonical_keeps_the_old_listener_when_rebind_is_rejected() {
    let root = tempfile::tempdir().expect("rebind-reject fixture");
    let dummy = write_current_build_dummy(root.path());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("canonical listen");
    let listen = listener.local_addr().expect("canonical address");
    tokio::spawn(serve_http_once(
        listener,
        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
            .to_owned(),
    ));
    let config = config_at(listen, root.path(), dummy.clone());
    let kept = try_canonical(&reqwest::Client::new(), &config, &health(true, Some(12)))
        .await
        .expect("rejected rebind must keep the old listener");
    assert_eq!(kept, None);
    kill_dummy(&dummy);
}

#[cfg(unix)]
#[tokio::test]
async fn try_canonical_restarts_on_canonical_after_warm_bind_misses() {
    let root = tempfile::tempdir().expect("canonical restart fixture");
    let dummy = write_current_build_dummy(root.path());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("canonical listen");
    let listen = listener.local_addr().expect("canonical address");
    let ephemeral = {
        let spare = TcpListener::bind("127.0.0.1:0").await.expect("ephemeral");
        spare.local_addr().expect("ephemeral address")
    };
    let body = format!(r#"{{"listen":"{ephemeral}"}}"#);
    tokio::spawn(serve_http_once(listener, http_ok(&body)));
    let config = config_at(listen, root.path(), dummy.clone());
    let promoted = try_canonical(&reqwest::Client::new(), &config, &health(true, Some(12)))
        .await
        .expect("canonical restart after warm miss");
    assert_eq!(promoted, Some(config.base_url()));
    kill_dummy(&dummy);
}

#[cfg(unix)]
#[tokio::test]
async fn try_canonical_bails_when_restart_on_canonical_never_readies() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().expect("canonical bail fixture");
    let real_dummy = write_current_build_dummy(root.path());
    let wrapper = root.path().join("claudex-agent-adapter-once");
    let counter = root.path().join("start-count");
    std::fs::write(&counter, "0").expect("start counter");
    let script = format!(
        "#!/bin/sh\n\
         count=$(cat '{counter}')\n\
         next=$((count + 1))\n\
         echo \"$next\" > '{counter}'\n\
         if [ \"$count\" -ge 1 ]; then\n\
           exit 0\n\
         fi\n\
         exec '{real}' \"$@\"\n",
        counter = counter.display(),
        real = real_dummy.display(),
    );
    std::fs::write(&wrapper, script).expect("once wrapper");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))
        .expect("wrapper executable");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("canonical listen");
    let listen = listener.local_addr().expect("canonical address");
    let ephemeral = {
        let spare = TcpListener::bind("127.0.0.1:0").await.expect("ephemeral");
        spare.local_addr().expect("ephemeral address")
    };
    let body = format!(r#"{{"listen":"{ephemeral}"}}"#);
    tokio::spawn(serve_http_once(listener, http_ok(&body)));
    let config = config_at(listen, root.path(), wrapper);
    let error = try_canonical(&reqwest::Client::new(), &config, &health(true, Some(12)))
        .await
        .expect_err("failed canonical restart must fail closed");
    assert!(
        error.to_string().contains("wait for promoted canonical"),
        "{error:#}"
    );
    kill_dummy(&real_dummy);
}

#[cfg(unix)]
#[tokio::test]
async fn try_canonical_bails_when_canonical_port_is_reoccupied_after_release() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().expect("canonical reoccupy fixture");
    let real_dummy = write_current_build_dummy(root.path());
    let wrapper = root.path().join("claudex-agent-adapter-delayed");
    let rebind_started = root.path().join("warm-rebind-started");
    let rebind_permit = root.path().join("warm-rebind-permit");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport CLAUDEX_TEST_REBIND_STARTED='{}'\nexport CLAUDEX_TEST_REBIND_PERMIT='{}'\nexec '{}' \"$@\"\n",
            rebind_started.display(),
            rebind_permit.display(),
            real_dummy.display(),
        ),
    )
    .expect("delayed dummy wrapper");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))
        .expect("delayed dummy executable");

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("canonical listener");
    let listen = listener.local_addr().expect("canonical address");
    let ephemeral = {
        let spare = TcpListener::bind("127.0.0.1:0").await.expect("ephemeral");
        spare.local_addr().expect("ephemeral address")
    };
    let response = http_ok(&format!(r#"{{"listen":"{ephemeral}"}}"#));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let server = spawn_reoccupying_canonical(
        listener,
        response,
        listen,
        rebind_started,
        rebind_permit,
        std::sync::Arc::clone(&stop),
    );

    let config = config_at(listen, root.path(), wrapper.clone());
    let error = try_canonical(&reqwest::Client::new(), &config, &health(true, Some(12)))
        .await
        .expect_err("reoccupied canonical port must fail closed");
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    server.join().expect("reoccupy server");
    kill_dummy(&real_dummy);
    kill_dummy(&wrapper);
    assert!(
        error.to_string().contains("wait for promoted canonical"),
        "{error:#}"
    );
}

#[tokio::test]
async fn restore_old_canonical_ignores_unreachable_retained_listeners() {
    let root = tempfile::tempdir().expect("restore fixture");
    let config = config_at(
        "127.0.0.1:1".parse().unwrap(),
        root.path(),
        PathBuf::from("/tmp/adapter"),
    );
    restore_old_canonical(
        &reqwest::Client::new(),
        &config,
        "127.0.0.1:1".parse().unwrap(),
    )
    .await;
}

#[test]
fn terminate_started_ignores_non_adapter_pids() {
    let root = tempfile::tempdir().expect("terminate fixture");
    let config = config_at(
        "127.0.0.1:8318".parse().unwrap(),
        root.path(),
        PathBuf::from("/tmp/claudex-agent-adapter"),
    );
    terminate_started(std::process::id(), &config);
}
