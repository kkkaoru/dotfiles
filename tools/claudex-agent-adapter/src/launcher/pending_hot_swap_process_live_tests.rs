use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use super::super::{LOCAL_TOKEN, ServiceConfig};
use super::process::{
    StartedWaiter, configure_detached_session, detached_waiter_group, process_command_line,
    request_waiter_stop, spawn_waiter, stop_waiter, terminate_waiter_group, waiter_is_alive,
};
use crate::agent_backend::{BackendKind, BackendRoute};
use crate::launcher::AdapterOptions;

fn config(root: &Path, listen: SocketAddr, executable: PathBuf) -> ServiceConfig {
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
        executable,
        log_path: root.join("adapter.log"),
        lock_path: root.join("adapter.lock"),
    }
}

fn write_sleep_adapter(root: &Path) -> PathBuf {
    let executable = root.join("claudex-agent-adapter");
    std::fs::write(&executable, "#!/bin/sh\nexec sleep 30\n").expect("adapter script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("executable adapter");
    }
    executable
}

fn wait_until(predicate: impl Fn() -> bool) {
    for _ in 0..50 {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn spawn_session_leader() -> std::process::Child {
    let mut command = Command::new("sleep");
    command.arg("30");
    configure_detached_session(&mut command);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn detached sleep")
}

#[cfg(unix)]
#[test]
fn detached_waiter_helpers_signal_a_live_session_leader() {
    let mut child = spawn_session_leader();
    let pid = child.id();
    wait_until(|| detached_waiter_group(pid).is_some());
    assert_eq!(detached_waiter_group(pid), Some(pid as i32));
    stop_waiter(pid, |_| true);
    request_waiter_stop(pid, |_| true);
    terminate_waiter_group(pid, |_| true);
    let _ = child.wait();
}

#[cfg(unix)]
#[test]
fn spawn_waiter_publishes_a_wait_idle_command_line() {
    let root = tempfile::tempdir().expect("waiter fixture");
    let executable = write_sleep_adapter(root.path());
    let config = config(
        root.path(),
        "127.0.0.1:8330".parse().expect("listen"),
        executable,
    );
    let waiter = spawn_waiter(&config).expect("spawn waiter");
    let pid = waiter.pid();
    wait_until(|| process_command_line(pid).is_some());
    let command = process_command_line(pid).expect("waiter command");
    assert!(
        command.contains("hot-swap") || command.contains("claudex-agent-adapter"),
        "{command}"
    );
    let _ = waiter_is_alive(pid);
    let _ = StartedWaiter::with_terminate(pid, |_| {});
    drop(waiter);
}

#[test]
fn process_command_line_skips_missing_pids() {
    assert!(process_command_line(u32::MAX).is_none());
    assert!(!waiter_is_alive(u32::MAX));
}
