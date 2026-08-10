use std::{ffi::OsString, net::SocketAddr};

use super::{NotifyForceGuard, notifications_enabled, parse_notify_env, run_internal};
use crate::launcher::launcher_logs;

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
