use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use super::super::{LOCAL_TOKEN, ServiceConfig};
use super::*;
use crate::agent_backend::{BackendKind, BackendRoute};
use crate::launcher::AdapterOptions;

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
}

#[test]
fn arms_once_per_live_build_and_respawns_stale_waiters() {
    let root = tempfile::tempdir().expect("pending hot-swap fixture");
    let config = config(root.path(), "127.0.0.1:8318".parse().expect("listen"));
    let events = super::super::macos_notify::TestEvents::capture();
    let first = arm_with(&config, |_| Ok(4242), |_| false).expect("first arm");
    assert_eq!(first, ArmOutcome::Spawned { pid: 4242 });
    assert_eq!(
        events.take(),
        vec![super::super::macos_notify::Event::WaitingForIdle {
            listen: "127.0.0.1:8318".to_owned(),
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            waiter_pid: 4242,
        }],
        "new waiter must notify that the build is waiting for idle"
    );
    let again = arm_with(&config, |_| Ok(4343), |pid| pid == 4242).expect("reuse live waiter");
    assert_eq!(again, ArmOutcome::AlreadyArmed { pid: 4242 });
    assert!(
        events.take().is_empty(),
        "already-armed waiter must not notify waiting again"
    );
    let respawn = arm_with(&config, |_| Ok(4444), |_| false).expect("respawn dead waiter");
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
    arm_with(&config, |_| Ok(5151), |_| false).expect("initial arm");
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
    let respawn = arm_with(&config, |_| Ok(5252), |pid| pid == 5151).expect("fingerprint change");
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
}

#[test]
fn stop_waiter_ignores_missing_self_and_dead_pids() {
    stop_waiter(0, |_| true);
    stop_waiter(std::process::id(), |_| true);
    stop_waiter(1, |_| false);
}
