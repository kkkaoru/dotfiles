use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use super::super::{LOCAL_TOKEN, ServiceConfig};
use super::process::detached_waiter_group;
use super::*;
use crate::agent_backend::{BackendKind, BackendRoute};
use crate::launcher::AdapterOptions;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

fn fake_waiter(pid: u32) -> StartedWaiter {
    StartedWaiter::with_terminate(pid, |_| {})
}

fn config(root: &Path, listen: SocketAddr) -> ServiceConfig {
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
        codex_config_fingerprint: "test-fingerprint".to_owned(),
        service_config_fingerprint: "service-fingerprint".to_owned(),
        executable: PathBuf::from("/tmp/claudex-agent-adapter"),
        log_path: root.join("adapter.log"),
        lock_path: root.join("adapter.lock"),
    }
}

#[test]
fn recognizes_wait_idle_command_lines_including_nohup() {
    assert!(is_wait_idle_command_line(
        "/Users/test/.cargo/bin/claudex-agent-adapter hot-swap --wait-idle --listen 127.0.0.1:8318"
    ));
    assert!(is_wait_idle_command_line(
        "nohup /Users/test/.cargo/bin/claudex-agent-adapter hot-swap --wait-idle --listen 127.0.0.1:8318"
    ));
    assert!(!is_wait_idle_command_line(
        "/Users/test/.cargo/bin/claudex-agent-adapter serve --listen 127.0.0.1:8318"
    ));
    assert!(!is_wait_idle_command_line(
        "/Users/test/.cargo/bin/claudex-agent-adapter hot-swap --listen 127.0.0.1:8318"
    ));
    assert!(!is_wait_idle_command_line(
        "/Users/test/.cargo/bin/claudex-agent-adapter launch --wait-idle --"
    ));
    assert!(!is_wait_idle_command_line(""));
    assert!(!is_wait_idle_command_line("   "));
    assert!(!is_wait_idle_command_line("nohup"));
}

#[test]
fn waiter_parser_rejects_missing_executable_and_disarm_skips_cleanup() {
    assert!(!is_wait_idle_command_line("nohup "));
    assert!(!is_wait_idle_command_line("other hot-swap --wait-idle"));
    assert!(!is_wait_idle_command_line("claudex-agent-adapter hot-swap"));
    let called = Arc::new(AtomicU32::new(0));
    let marker = Arc::clone(&called);
    let waiter = StartedWaiter::with_terminate(42, move |_| {
        marker.fetch_add(1, Ordering::Relaxed);
    });
    assert_eq!(waiter.disarm(), 42);
    assert_eq!(called.load(Ordering::Relaxed), 0);
}

#[cfg(unix)]
#[test]
fn detached_waiter_group_rejects_invalid_and_non_detached_pids() {
    assert_eq!(detached_waiter_group(0), None);
    assert_eq!(detached_waiter_group(std::process::id()), None);
    assert_eq!(detached_waiter_group(i32::MAX as u32 + 1), None);
    request_waiter_stop(0, |_| true);
    request_waiter_stop(std::process::id(), |_| true);
    request_waiter_stop(i32::MAX as u32 + 1, |_| true);
    request_waiter_stop(99_999_999, |_| true);
    terminate_waiter_group(99_999_999, |_| true);
}

#[test]
fn arms_once_per_live_build_and_respawns_stale_waiters() {
    let root = tempfile::tempdir().expect("pending hot-swap fixture");
    let config = config(root.path(), "127.0.0.1:8318".parse().expect("listen"));
    let events = super::super::macos_notify::TestEvents::capture();
    let first = arm_with(&config, |_| Ok(fake_waiter(4242)), |_| false).expect("first arm");
    assert_eq!(first, ArmOutcome::Spawned { pid: 4242 });
    assert!(
        events.take().is_empty(),
        "arming a waiter must not macOS-notify; only swap-complete alerts"
    );
    let again =
        arm_with(&config, |_| Ok(fake_waiter(4343)), |pid| pid == 4242).expect("reuse live waiter");
    assert_eq!(again, ArmOutcome::AlreadyArmed { pid: 4242 });
    assert!(
        events.take().is_empty(),
        "already-armed waiter must not notify waiting again"
    );
    let respawn =
        arm_with(&config, |_| Ok(fake_waiter(4444)), |_| false).expect("respawn dead waiter");
    assert_eq!(respawn, ArmOutcome::Spawned { pid: 4444 });
    assert!(
        events.take().is_empty(),
        "respawning the same build must not notify waiting again"
    );
    let state = read_state_for_tests(&config)
        .expect("read pending")
        .expect("pending after respawn");
    assert_eq!(state.pid, 4444);
    assert_eq!(state.build_id, env!("CLAUDEX_BUILD_ID"));
    clear_if_current(&config);
    assert!(read_state_for_tests(&config).expect("cleared").is_none());
}

#[test]
fn pending_state_round_trips_build_and_pid() {
    let state = PendingHotSwap {
        build_id: "build".to_owned(),
        service_config_fingerprint: "service".to_owned(),
        pid: 42,
    };
    let encoded = serde_json::to_string(&state).expect("encode");
    let decoded: PendingHotSwap = serde_json::from_str(&encoded).expect("decode");
    assert_eq!(decoded, state);
}

#[test]
fn arm_outcome_pid_matches_each_variant() {
    assert_eq!(ArmOutcome::AlreadyArmed { pid: 7 }.pid(), 7);
    assert_eq!(ArmOutcome::Spawned { pid: 9 }.pid(), 9);
}

#[test]
fn respawns_when_fingerprint_changes_and_ignores_foreign_builds() {
    let root = tempfile::tempdir().expect("pending hot-swap fixture");
    let config = config(root.path(), "127.0.0.1:8319".parse().expect("listen"));
    arm_with(&config, |_| Ok(fake_waiter(5151)), |_| false).expect("initial arm");
    let path = state_path(&config).expect("state path");
    write_state(
        &path,
        &PendingHotSwap {
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            service_config_fingerprint: "other-fingerprint".to_owned(),
            pid: 5151,
        },
    )
    .expect("stale fingerprint");
    let respawn = arm_with(&config, |_| Ok(fake_waiter(5252)), |pid| pid == 5151)
        .expect("fingerprint change");
    assert_eq!(respawn, ArmOutcome::Spawned { pid: 5252 });

    write_state(
        &path,
        &PendingHotSwap {
            build_id: "other-build".to_owned(),
            service_config_fingerprint: config.service_config_fingerprint.clone(),
            pid: 5252,
        },
    )
    .expect("foreign build");
    clear_if_current(&config);
    assert_eq!(
        read_state_for_tests(&config)
            .expect("foreign build remains")
            .expect("pending")
            .build_id,
        "other-build"
    );
    disarm(&config);
    assert!(
        read_state_for_tests(&config)
            .expect("disarm removes any waiter")
            .is_none()
    );
}

#[test]
fn waiter_stop_helpers_ignore_missing_self_and_dead_pids() {
    stop_waiter(0, |_| true);
    stop_waiter(std::process::id(), |_| true);
    stop_waiter(1, |_| false);
    request_waiter_stop(0, |_| true);
    request_waiter_stop(std::process::id(), |_| true);
    request_waiter_stop(1, |_| false);
    terminate_waiter_group(1, |_| false);
}

#[test]
fn arm_respawns_when_existing_waiter_build_differs() {
    let root = tempfile::tempdir().expect("pending hot-swap fixture");
    let config = config(root.path(), "127.0.0.1:8321".parse().expect("listen"));
    let path = state_path(&config).expect("state path");
    write_state(
        &path,
        &PendingHotSwap {
            build_id: "other-build".to_owned(),
            service_config_fingerprint: config.service_config_fingerprint.clone(),
            pid: 6161,
        },
    )
    .expect("foreign build");
    let respawn =
        arm_with(&config, |_| Ok(fake_waiter(6262)), |pid| pid == 6161).expect("foreign build arm");
    assert_eq!(respawn, ArmOutcome::Spawned { pid: 6262 });
}

#[test]
fn state_write_failure_terminates_the_new_waiter_and_publishes_nothing() {
    let root = tempfile::tempdir().expect("pending hot-swap failure fixture");
    let config = config(root.path(), "127.0.0.1:8322".parse().expect("listen"));
    let terminated = Arc::new(AtomicU32::new(0));
    let observed = Arc::clone(&terminated);
    let _failure = FailStateWrite::arm();

    let error = arm_with(
        &config,
        move |_| {
            let terminate = move |pid| observed.store(pid, Ordering::SeqCst);
            Ok(StartedWaiter::with_terminate(7373, terminate))
        },
        |_| false,
    )
    .expect_err("injected state publication failure must fail arm");

    assert!(
        error
            .to_string()
            .contains("injected pending hot-swap state")
    );
    assert_eq!(terminated.load(Ordering::SeqCst), 7373);
    assert!(read_state_for_tests(&config).expect("state read").is_none());
}

#[test]
fn started_waiter_disarms_only_after_publication() {
    let terminated = std::cell::Cell::new(0);
    let started = StartedWaiter::with_terminate(42, |pid| terminated.set(pid));
    assert_eq!(started.pid(), 42);
    assert_eq!(started.disarm(), 42);
    assert_eq!(terminated.get(), 0);
}

#[tokio::test]
async fn started_waiter_terminates_when_a_post_spawn_future_is_cancelled() {
    let terminated = Arc::new(AtomicU32::new(0));
    let observed = Arc::clone(&terminated);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let terminate = move |pid| observed.store(pid, Ordering::SeqCst);
        let _started = StartedWaiter::with_terminate(77, terminate);
        let _ = ready_tx.send(());
        std::future::pending::<()>().await;
    });
    ready_rx.await.expect("guarded future started");
    task.abort();
    let _ = task.await;
    assert_eq!(terminated.load(Ordering::SeqCst), 77);
}

#[test]
fn clear_and_disarm_are_noops_when_log_has_no_parent() {
    let mut cfg = config(
        tempfile::tempdir().expect("orphan log fixture").path(),
        "127.0.0.1:8320".parse().expect("listen"),
    );
    cfg.log_path = PathBuf::from("/");
    clear_if_current(&cfg);
    disarm(&cfg);
}
