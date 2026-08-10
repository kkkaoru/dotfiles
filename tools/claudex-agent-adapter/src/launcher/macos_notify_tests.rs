use std::{fs, net::SocketAddr};

use super::*;

fn listen() -> SocketAddr {
    "127.0.0.1:8318".parse().expect("listen")
}

fn waiting(build: &str, waiter_pid: u32) -> Event {
    Event::WaitingForIdle {
        listen: listen().to_string(),
        build_id: build.to_owned(),
        waiter_pid,
    }
}

fn complete(build: &str) -> Event {
    Event::SwapComplete {
        listen: listen().to_string(),
        build_id: build.to_owned(),
    }
}

fn live(build: &str) -> Event {
    Event::LiveReady {
        listen: "127.0.0.1:62789".to_owned(),
        build_id: build.to_owned(),
        waiting: listen().to_string(),
    }
}

#[test]
fn waiting_notification_describes_build_listen_and_waiter() {
    let notification = notification(&waiting("abc123", 4242));
    assert_eq!(notification.title, TITLE);
    assert_eq!(notification.subtitle, WAITING_SUBTITLE);
    assert!(notification.body.contains("127.0.0.1:8318"));
    assert!(notification.body.contains("abc123"));
    assert!(notification.body.contains("4242"));
    assert!(notification.body.contains("待機中"));
}

#[test]
fn live_ready_notification_describes_live_listen_build_and_waiting_port() {
    let notification = notification(&live("abc123"));
    assert_eq!(notification.title, TITLE);
    assert_eq!(notification.subtitle, LIVE_SUBTITLE);
    assert!(notification.body.contains("127.0.0.1:62789"));
    assert!(notification.body.contains("abc123"));
    assert!(notification.body.contains("127.0.0.1:8318"));
    assert!(notification.body.contains("即時利用"));
}

#[test]
fn swap_complete_notification_describes_build_and_listen() {
    let notification = notification(&complete("abc123"));
    assert_eq!(notification.title, TITLE);
    assert_eq!(notification.subtitle, COMPLETE_SUBTITLE);
    assert!(notification.body.contains("127.0.0.1:8318"));
    assert!(notification.body.contains("abc123"));
    assert!(notification.body.contains("差し替えました"));
}

#[test]
fn applescript_escapes_quotes_and_backslashes() {
    assert_eq!(escape_applescript(r#"a"b\c"#), r#"a\"b\\c"#);
}

#[test]
fn osascript_command_uses_display_notification() {
    let notification = notification(&Event::SwapComplete {
        listen: "127.0.0.1:8318".to_owned(),
        build_id: r#"q"w"#.to_owned(),
    });
    let command = osascript_command(&notification);
    assert_eq!(command.get_program(), osascript_program().as_os_str());
    let args: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(args[0], "-e");
    assert!(args[1].starts_with("display notification \""));
    assert!(args[1].contains("with title \"claudex\""));
    assert!(args[1].contains("subtitle \"差し替え完了\""));
    assert!(args[1].contains(r#"q\"w"#));
}

#[test]
fn deliver_status_accepts_success_and_rejects_failure() {
    use std::os::unix::process::ExitStatusExt;
    deliver_status(ExitStatus::from_raw(0)).expect("success");
    let error = deliver_status(ExitStatus::from_raw(1 << 8)).expect_err("nonzero");
    assert!(error.to_string().contains("osascript exited"));
}

#[test]
fn same_build_and_kind_are_not_emitted_twice() {
    let first_waiting = waiting("abc", 1);
    let first_complete = complete("abc");
    assert!(
        should_emit(&first_waiting, None),
        "first waiting must notify"
    );
    assert!(
        !should_emit(&first_waiting, Some(&LastNotify::from(&first_waiting))),
        "duplicate waiting for the same build must not notify"
    );
    assert!(
        should_emit(&live("abc"), Some(&LastNotify::from(&first_waiting))),
        "live ready after waiting must still notify"
    );
    assert!(
        !should_emit(&live("abc"), Some(&LastNotify::from(&live("abc")))),
        "duplicate live ready for the same build must not notify"
    );
    assert!(
        should_emit(&first_complete, Some(&LastNotify::from(&live("abc")))),
        "swap complete after live ready must still notify"
    );
    assert!(
        should_emit(&first_complete, Some(&LastNotify::from(&first_waiting))),
        "swap complete after waiting must still notify"
    );
    assert!(
        !should_emit(&first_complete, Some(&LastNotify::from(&first_complete))),
        "duplicate swap complete for the same build must not notify"
    );
    assert!(
        should_emit(&waiting("def", 2), Some(&LastNotify::from(&first_complete))),
        "a newer build must notify waiting again"
    );
}

#[test]
fn post_emits_waiting_then_complete_once_each() {
    let root = tempfile::tempdir().expect("notify cache");
    let listen = listen();
    let events = TestEvents::capture();
    post(root.path(), &listen, waiting("abc", 9));
    post(root.path(), &listen, waiting("abc", 10));
    post(root.path(), &listen, live("abc"));
    post(root.path(), &listen, live("abc"));
    post(root.path(), &listen, complete("abc"));
    post(root.path(), &listen, complete("abc"));
    assert_eq!(
        events.take(),
        vec![waiting("abc", 9), live("abc"), complete("abc")],
        "busy fallback must notify live ready, then one complete after canonical replace"
    );
}

#[test]
fn post_records_the_event_even_when_osascript_cannot_start() {
    let root = tempfile::tempdir().expect("notify cache");
    let spawn = TestSpawn::arm(|_| {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "osascript",
        ))
    });
    post(root.path(), &listen(), waiting("dead", 1));
    assert_eq!(spawn.take_events(), vec![waiting("dead", 1)]);
}

#[test]
fn deliver_fails_when_osascript_exits_nonzero() {
    use std::os::unix::process::ExitStatusExt;
    let _spawn = TestSpawn::arm(|_| Ok(ExitStatus::from_raw(1 << 8)));
    let error = deliver(&notification(&complete("dead"))).expect_err("nonzero osascript");
    assert!(error.to_string().contains("osascript exited"));
}

#[test]
fn live_ready_is_a_no_op_when_listen_did_not_move() {
    let root = tempfile::tempdir().expect("notify cache");
    let listen = listen();
    let config = super::super::ServiceConfig {
        options: crate::launcher::AdapterOptions {
            routes: Vec::new(),
            listen,
            model: "test-model".to_owned(),
            subscription_max_processes: 20,
            subscription_timeout_minutes: 120,
            subagent_hard_timeout_seconds: None,
            model_catalog: crate::provider_config::ModelCatalog::default(),
        },
        token: super::super::LOCAL_TOKEN.to_owned(),
        codex_config_fingerprint: "fp".to_owned(),
        service_config_fingerprint: "svc".to_owned(),
        executable: std::path::PathBuf::from("/tmp/claudex-agent-adapter"),
        log_path: root.path().join("adapter.log"),
        lock_path: root.path().join("adapter.lock"),
    };
    let events = TestEvents::capture();
    live_ready(&config, listen);
    assert!(events.take().is_empty());
}

#[test]
fn notify_helpers_return_early_when_log_path_has_no_parent() {
    let listen = listen();
    let config = super::super::ServiceConfig {
        options: crate::launcher::AdapterOptions {
            routes: Vec::new(),
            listen,
            model: "test-model".to_owned(),
            subscription_max_processes: 20,
            subscription_timeout_minutes: 120,
            subagent_hard_timeout_seconds: None,
            model_catalog: crate::provider_config::ModelCatalog::default(),
        },
        token: super::super::LOCAL_TOKEN.to_owned(),
        codex_config_fingerprint: "fp".to_owned(),
        service_config_fingerprint: "svc".to_owned(),
        executable: std::path::PathBuf::from("/tmp/claudex-agent-adapter"),
        // Empty path has no parent, so waiting/live/swap helpers must no-op.
        log_path: std::path::PathBuf::new(),
        lock_path: std::path::PathBuf::from("adapter.lock"),
    };
    let events = TestEvents::capture();
    waiting_for_idle(&config, 4242);
    live_ready(&config, "127.0.0.1:62789".parse().expect("live listen"));
    swap_complete(&config);
    assert!(
        events.take().is_empty(),
        "notify helpers must not emit when cache parent is absent"
    );
}

#[test]
fn corrupt_dedup_state_does_not_block_the_next_notification() {
    let root = tempfile::tempdir().expect("notify cache");
    let listen = listen();
    fs::write(
        launcher_logs::hot_swap_notify_path(root.path(), &listen),
        "{not-json",
    )
    .expect("corrupt dedup state");
    assert!(read_last(root.path(), &listen).is_none());
    let events = TestEvents::capture();
    post(root.path(), &listen, waiting("abc", 3));
    assert_eq!(events.take(), vec![waiting("abc", 3)]);
}
