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
fn retains_only_busy_sessions_when_the_new_health_field_is_present() {
    let mut health = health(true, Some(12));
    health.busy_claude_session_ids = vec!["busy-a".to_owned()];
    health.active_claude_session_ids = vec!["busy-a".to_owned(), "idle-tui".to_owned()];
    assert_eq!(retained_session_ids(&health), ["busy-a"]);
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
    release_previous(&config, std::process::id());
    let _ = publish_promoted(&config, 99, 12, config.options.listen, 0);
    let _ = publish_promoted(&config, 99, 12, config.options.listen, 2);
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

#[cfg(unix)]
#[tokio::test]
async fn try_canonical_fails_closed_when_warm_start_never_becomes_ready() {
    use std::os::unix::fs::PermissionsExt;
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
    let error = try_canonical(&reqwest::Client::new(), &config, &health(true, Some(12)))
        .await
        .expect_err("warm-start must fail closed");
    assert!(
        error.to_string().contains("wait for warm-start"),
        "{error:#}"
    );
}

#[tokio::test]
async fn try_canonical_reports_retained_state_write_failure() {
    let root = tempfile::tempdir().expect("retained write fixture");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("canonical listen");
    let listen = listener.local_addr().expect("canonical address");
    drop(listener);
    let config = config_at(listen, root.path(), PathBuf::from("/tmp/adapter"));
    std::fs::create_dir(
        root.path()
            .join(format!("retained.{}.json", config.options.listen.port())),
    )
    .expect("block retained state path");
    let error = try_canonical(&reqwest::Client::new(), &config, &health(true, Some(12)))
        .await
        .expect_err("retained state write must fail");
    assert!(
        error.to_string().contains("state") || error.to_string().contains("directory"),
        "{error:#}"
    );
}

#[tokio::test]
async fn request_rebind_parses_success_and_skips_unreachable_listeners() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("rebind listener");
    let listen = listener.local_addr().expect("rebind address");
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut buf = vec![0; 4096];
        let _ = stream.read(&mut buf).await;
        let body = r#"{"listen":"127.0.0.1:65100"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
    });
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
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut buf = vec![0; 4096];
        let _ = stream.read(&mut buf).await;
        let response = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
        let _ = stream.write_all(response.as_bytes()).await;
    });
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
    let became_free = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if listen_is_free(listen) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or(false);
    assert!(became_free, "port should become free after the listener drops");
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
#[tokio::test]
async fn try_canonical_keeps_the_old_listener_when_rebind_is_rejected() {
    let root = tempfile::tempdir().expect("rebind-reject fixture");
    let dummy = write_current_build_dummy(root.path());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("canonical listen");
    let listen = listener.local_addr().expect("canonical address");
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut buf = vec![0; 4096];
        let _ = stream.read(&mut buf).await;
        let response = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
        let _ = stream.write_all(response.as_bytes()).await;
    });
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
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut buf = vec![0; 4096];
        let _ = stream.read(&mut buf).await;
        let body = format!(r#"{{"listen":"{ephemeral}"}}"#);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
    });
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
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut buf = vec![0; 4096];
        let _ = stream.read(&mut buf).await;
        let body = format!(r#"{{"listen":"{ephemeral}"}}"#);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
    });
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
