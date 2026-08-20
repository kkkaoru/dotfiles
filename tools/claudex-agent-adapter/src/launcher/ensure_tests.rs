use std::path::PathBuf;
use std::time::Duration;

use super::handover::ServiceState;
use super::*;
use crate::agent_backend::{BackendKind, BackendRoute};
use crate::launcher::{AdapterOptions, LOCAL_TOKEN, ServiceConfig};

fn config(root: &std::path::Path) -> ServiceConfig {
    ServiceConfig {
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
        log_path: root.join("adapter.log"),
        lock_path: root.join("adapter.lock"),
    }
}

#[test]
fn wait_idle_poll_stays_snappy_while_listeners_are_busy() {
    assert_eq!(WAIT_IDLE_POLL_INTERVAL, Duration::from_millis(100));
    assert!(
        WAIT_IDLE_POLL_INTERVAL < Duration::from_secs(1),
        "busy wait-idle must not sleep a full second between rechecks"
    );
}

#[test]
fn wait_idle_inspect_pause_guard_arms_without_changing_poll_interval() {
    let _pause = WaitIdleInspectPause::arm(Duration::from_millis(5));
    assert_eq!(WAIT_IDLE_POLL_INTERVAL, Duration::from_millis(100));
}

#[test]
fn should_retry_idle_replace_respects_optional_limits() {
    assert!(should_retry_idle_replace(0, None));
    assert!(should_retry_idle_replace(9, None));
    assert!(should_retry_idle_replace(0, Some(0)));
    assert!(!should_retry_idle_replace(1, Some(0)));
    assert!(should_retry_idle_replace(2, Some(2)));
    assert!(!should_retry_idle_replace(3, Some(2)));
}

#[test]
fn listener_was_replaced_detects_replace_states() {
    assert!(listener_was_replaced(&ServiceState::Replace {
        pid: Some(1),
        recovery_generation: None,
    }));
    assert!(!listener_was_replaced(&ServiceState::Reuse));
    assert!(!listener_was_replaced(&ServiceState::Start));
    assert!(!listener_was_replaced(&ServiceState::Defer {
        pid: None,
        active_http_requests: 0,
        active_provider_turns: 0,
        active_subagents: 0,
    }));
}

#[test]
fn recovery_snapshot_is_missing_detects_not_found_io_errors() {
    let missing = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "missing snapshot",
    ));
    assert!(recovery_snapshot_is_missing(&missing));
    let nested = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "inner missing",
    ))
    .context("validate recovery generation");
    assert!(recovery_snapshot_is_missing(&nested));
    let other = anyhow::anyhow!("unrelated");
    assert!(!recovery_snapshot_is_missing(&other));
    let other_io = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "denied",
    ));
    assert!(!recovery_snapshot_is_missing(&other_io));
}

#[test]
fn live_listener_helpers_ignore_invalid_url_and_missing_state() {
    let root = tempfile::tempdir().expect("listener helper fixture");
    let config = config(root.path());
    notify_live_listener(&config, "not-a-listen");
    log_live_listener(&config);
}

#[tokio::test]
async fn try_defer_live_update_skips_ineligible_health() {
    use std::{
        net::TcpListener,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };

    let root = tempfile::tempdir().expect("defer live fixture");
    let listener = TcpListener::bind("127.0.0.1:0").expect("health listener");
    let mut config = config(root.path());
    config.options.listen = listener.local_addr().expect("health address");
    let body = serde_json::json!({
        "status": "ok",
        "pid": 42,
        "protocol_version": crate::ADAPTER_PROTOCOL_VERSION,
        "build_id": env!("CLAUDEX_BUILD_ID"),
        "codex_config_fingerprint": config.codex_config_fingerprint,
        "service_config_fingerprint": "other-service",
        "subscription_max_processes": 20,
        "subscription_timeout_minutes": 120,
        "listener_handover": true,
    })
    .to_string();
    let stopped = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stopped);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let server = thread::spawn(move || serve_ineligible_health(listener, response, stop_flag));

    let result = try_defer_live_update(&config, &reqwest::Client::new(), Some(42))
        .await
        .expect("ineligible health is not an error");
    assert_eq!(result, None);
    stopped.store(true, Ordering::SeqCst);
    server.join().expect("health server");
}

fn serve_ineligible_health(
    listener: std::net::TcpListener,
    response: String,
    stop_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::{sync::atomic::Ordering, thread, time::Duration};
    listener
        .set_nonblocking(true)
        .expect("nonblocking health listener");
    while !stop_flag.load(Ordering::SeqCst) {
        if accept_ineligible_health(&listener, &response) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
}

#[expect(
    clippy::excessive_nesting,
    reason = "test-only HTTP framing parser remains local and explicit"
)]
fn read_complete_health_request(stream: &mut std::net::TcpStream) {
    use std::io::Read;

    const MAX_REQUEST_BYTES: usize = 64 * 1024;
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).expect("read health request");
        assert!(read > 0, "health request ended before completion");
        request.extend_from_slice(&chunk[..read]);
        assert!(
            request.len() <= MAX_REQUEST_BYTES,
            "health request exceeded fixture limit"
        );
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = std::str::from_utf8(&request[..header_end]).expect("health request headers");
        let content_length = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find_map(|(name, value)| {
                name.eq_ignore_ascii_case("content-length").then(|| {
                    value
                        .trim()
                        .parse::<usize>()
                        .expect("health content length")
                })
            })
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            return;
        }
    }
}

fn accept_ineligible_health(listener: &std::net::TcpListener, response: &str) -> bool {
    use std::io::Write;
    let (mut stream, _) = match listener.accept() {
        Ok(accepted) => accepted,
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return false,
        Err(error) => panic!("health accept: {error}"),
    };
    stream
        .set_nonblocking(false)
        .expect("blocking health request stream");
    read_complete_health_request(&mut stream);
    let _ = stream.write_all(response.as_bytes());
    true
}
