use std::{
    ffi::OsString,
    net::SocketAddr,
    sync::{Mutex, MutexGuard},
};

use super::{
    DelegateForceGuard, NotifyForceGuard, delegate_complete_notify, interpret_delegate_status,
    notifications_enabled, parse_notify_env, post, run_internal,
};
use crate::launcher::{installed_adapter, launcher_logs, macos_notify::Event};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn interpret_delegate_status_covers_success_failure_and_spawn_errors() {
    assert!(interpret_delegate_status(Ok(
        std::process::Command::new("true")
            .status()
            .expect("true")
    )));
    assert!(!interpret_delegate_status(Ok(
        std::process::Command::new("false")
            .status()
            .expect("false")
    )));
    assert!(!interpret_delegate_status(Err(std::io::Error::other(
        "spawn failed"
    ))));
}

#[test]
fn notifications_enabled_respects_test_force_guard() {
    assert!(notifications_enabled(), "tests default to enabled");
    let _disabled = NotifyForceGuard::push(false);
    assert!(!notifications_enabled(), "force guard must opt out");
    drop(_disabled);
    assert!(notifications_enabled(), "force guard restore re-enables notify");
}

#[test]
fn parse_notify_env_honors_explicit_tokens() {
    for value in ["0", "false", "FALSE", "no", "NO"] {
        assert!(!parse_notify_env(Some(value)), "{value} must opt out");
    }
    for value in ["1", "true", "TRUE", "yes", "YES"] {
        assert!(parse_notify_env(Some(value)), "{value} must opt in");
    }
}

#[test]
fn run_internal_posts_swap_complete_in_process() {
    let root = tempfile::tempdir().expect("notify cache");
    let listen = "127.0.0.1:8318";
    let listen_addr: SocketAddr = listen.parse().expect("listen");
    run_internal(vec![
        OsString::from("claudex-agent-adapter"),
        OsString::from("__internal-notify"),
        OsString::from("complete"),
        OsString::from(root.path()),
        OsString::from(listen),
        OsString::from("delegated-build"),
    ])
    .expect("internal notify");
    let state_path = launcher_logs::hot_swap_notify_path(root.path(), &listen_addr);
    let state = std::fs::read_to_string(&state_path).expect("dedup state");
    assert!(
        state.contains("delegated-build"),
        "internal notify must persist dedup state: {state}"
    );
}

#[test]
fn run_internal_rejects_missing_or_invalid_arguments() {
    let error = run_internal(vec![OsString::from("argv0")]).expect_err("missing flag");
    assert!(error.to_string().contains("missing __internal-notify"));

    let error = run_internal(vec![
        OsString::from("argv0"),
        OsString::from("not-internal"),
    ])
    .expect_err("wrong flag");
    assert!(error.to_string().contains("expected __internal-notify"));

    let error = run_internal(vec![
        OsString::from("argv0"),
        OsString::from("__internal-notify"),
        OsString::from("waiting"),
    ])
    .expect_err("unsupported kind");
    assert!(error.to_string().contains("unsupported notify kind"));

    let error = run_internal(vec![
        OsString::from("argv0"),
        OsString::from("__internal-notify"),
        OsString::from("complete"),
        OsString::from("/tmp/cache"),
        OsString::from("not-a-socket"),
        OsString::from("build"),
    ])
    .expect_err("bad listen");
    assert!(error.to_string().contains("parse notify listen"));
}

#[test]
fn delegate_complete_notify_skips_non_complete_events() {
    let root = tempfile::tempdir().expect("notify cache");
    assert!(!delegate_complete_notify(
        root.path(),
        &Event::WaitingForIdle {
            listen: "127.0.0.1:8318".to_owned(),
            waiter_pid: 1,
            build_id: "build".to_owned(),
        }
    ));
}

#[test]
fn delegate_complete_notify_returns_false_without_an_executable() {
    let _lock = env_lock();
    let previous_home = std::env::var_os("HOME");
    let previous_cargo = std::env::var_os("CARGO_HOME");
    let previous_adapter = std::env::var_os(installed_adapter::ADAPTER_EXECUTABLE_ENV);
    let previous_notify = std::env::var_os(installed_adapter::NOTIFY_IN_PROCESS_ENV);
    let root = tempfile::tempdir().expect("empty home");
    unsafe {
        std::env::set_var("HOME", root.path());
        std::env::set_var("CARGO_HOME", root.path().join(".cargo"));
        std::env::remove_var(installed_adapter::ADAPTER_EXECUTABLE_ENV);
        std::env::remove_var(installed_adapter::NOTIFY_IN_PROCESS_ENV);
    }
    assert!(!delegate_complete_notify(
        root.path(),
        &Event::SwapComplete {
            listen: "127.0.0.1:8318".to_owned(),
            build_id: "build".to_owned(),
        }
    ));
    restore_os("HOME", previous_home);
    restore_os("CARGO_HOME", previous_cargo);
    restore_os(installed_adapter::ADAPTER_EXECUTABLE_ENV, previous_adapter);
    restore_os(installed_adapter::NOTIFY_IN_PROCESS_ENV, previous_notify);
}

#[cfg(unix)]
#[test]
fn delegate_complete_notify_reports_success_and_failure_statuses() {
    let _lock = env_lock();
    let previous_home = std::env::var_os("HOME");
    let previous_cargo = std::env::var_os("CARGO_HOME");
    let previous_adapter = std::env::var_os(installed_adapter::ADAPTER_EXECUTABLE_ENV);
    let previous_notify = std::env::var_os(installed_adapter::NOTIFY_IN_PROCESS_ENV);
    let root = tempfile::tempdir().expect("delegate fixture");
    let ok = root.path().join("ok.sh");
    let fail = root.path().join("fail.sh");
    std::fs::write(&ok, "#!/bin/sh\nexit 0\n").expect("ok script");
    std::fs::write(&fail, "#!/bin/sh\nexit 1\n").expect("fail script");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&ok, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&fail, std::fs::Permissions::from_mode(0o755)).unwrap();

    unsafe {
        std::env::set_var("HOME", root.path());
        std::env::set_var(installed_adapter::ADAPTER_EXECUTABLE_ENV, &ok);
        std::env::remove_var(installed_adapter::NOTIFY_IN_PROCESS_ENV);
    }
    assert!(delegate_complete_notify(
        root.path(),
        &Event::SwapComplete {
            listen: "127.0.0.1:8318".to_owned(),
            build_id: "ok-build".to_owned(),
        }
    ));

    unsafe {
        std::env::set_var(installed_adapter::ADAPTER_EXECUTABLE_ENV, &fail);
    }
    assert!(!delegate_complete_notify(
        root.path(),
        &Event::SwapComplete {
            listen: "127.0.0.1:8318".to_owned(),
            build_id: "fail-build".to_owned(),
        }
    ));

    unsafe {
        std::env::set_var(
            installed_adapter::ADAPTER_EXECUTABLE_ENV,
            root.path().join("missing-delegate"),
        );
    }
    assert!(!delegate_complete_notify(
        root.path(),
        &Event::SwapComplete {
            listen: "127.0.0.1:8318".to_owned(),
            build_id: "missing-build".to_owned(),
        }
    ));

    restore_os("HOME", previous_home);
    restore_os("CARGO_HOME", previous_cargo);
    restore_os(installed_adapter::ADAPTER_EXECUTABLE_ENV, previous_adapter);
    restore_os(installed_adapter::NOTIFY_IN_PROCESS_ENV, previous_notify);
}

#[cfg(unix)]
#[test]
fn post_returns_early_when_delegation_succeeds() {
    let _lock = env_lock();
    let previous_adapter = std::env::var_os(installed_adapter::ADAPTER_EXECUTABLE_ENV);
    let previous_notify = std::env::var_os(installed_adapter::NOTIFY_IN_PROCESS_ENV);
    let root = tempfile::tempdir().expect("post delegate fixture");
    let ok = root.path().join("ok.sh");
    std::fs::write(&ok, "#!/bin/sh\nexit 0\n").expect("ok script");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&ok, std::fs::Permissions::from_mode(0o755)).unwrap();
    unsafe {
        std::env::set_var(installed_adapter::ADAPTER_EXECUTABLE_ENV, &ok);
        std::env::remove_var(installed_adapter::NOTIFY_IN_PROCESS_ENV);
    }
    let _delegate = DelegateForceGuard::push(true);
    let listen: SocketAddr = "127.0.0.1:8318".parse().expect("listen");
    post(
        root.path(),
        &listen,
        Event::SwapComplete {
            listen: listen.to_string(),
            build_id: "delegated".to_owned(),
        },
    );
    assert!(
        !launcher_logs::hot_swap_notify_path(root.path(), &listen).exists(),
        "successful delegation must skip in-process post"
    );
    restore_os(installed_adapter::ADAPTER_EXECUTABLE_ENV, previous_adapter);
    restore_os(installed_adapter::NOTIFY_IN_PROCESS_ENV, previous_notify);
}

#[cfg(unix)]
#[test]
fn delegate_complete_notify_handles_non_utf8_cache_and_spawn_errors() {
    use std::ffi::OsString;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let _lock = env_lock();
    let previous_adapter = std::env::var_os(installed_adapter::ADAPTER_EXECUTABLE_ENV);
    let previous_notify = std::env::var_os(installed_adapter::NOTIFY_IN_PROCESS_ENV);
    let root = tempfile::tempdir().expect("spawn error fixture");
    let bogus = root.path().join("bogus-bin");
    std::fs::write(&bogus, b"not-an-executable-image").expect("bogus bin");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bogus, std::fs::Permissions::from_mode(0o755)).unwrap();
    unsafe {
        std::env::set_var(installed_adapter::ADAPTER_EXECUTABLE_ENV, &bogus);
        std::env::remove_var(installed_adapter::NOTIFY_IN_PROCESS_ENV);
    }
    assert!(!delegate_complete_notify(
        root.path(),
        &Event::SwapComplete {
            listen: "127.0.0.1:8318".to_owned(),
            build_id: "bogus-build".to_owned(),
        }
    ));

    let mut bytes = root.path().as_os_str().as_bytes().to_vec();
    bytes.push(0xff);
    let non_utf8 = std::path::PathBuf::from(OsString::from_vec(bytes));
    assert!(!delegate_complete_notify(
        &non_utf8,
        &Event::SwapComplete {
            listen: "127.0.0.1:8318".to_owned(),
            build_id: "non-utf8".to_owned(),
        }
    ));

    restore_os(installed_adapter::ADAPTER_EXECUTABLE_ENV, previous_adapter);
    restore_os(installed_adapter::NOTIFY_IN_PROCESS_ENV, previous_notify);
}

fn restore_os(key: &str, value: Option<OsString>) {
    match value {
        Some(value) => unsafe { std::env::set_var(key, value) },
        None => unsafe { std::env::remove_var(key) },
    }
}
