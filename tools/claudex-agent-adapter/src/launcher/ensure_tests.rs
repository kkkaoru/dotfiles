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
