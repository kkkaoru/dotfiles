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
    complete_on(listen(), build)
}

fn complete_on(addr: SocketAddr, build: &str) -> Event {
    Event::SwapComplete {
        listen: addr.to_string(),
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
    assert_eq!(notification.title, "claudex · 127.0.0.1:8318");
    assert_eq!(notification.subtitle, "ビルド完了・待機中 · build abc123");
    assert!(notification.body.contains("127.0.0.1:8318"));
    assert!(notification.body.contains("abc123"));
    assert!(notification.body.contains("4242"));
    assert!(notification.body.contains("待機中"));
}

#[test]
fn live_ready_notification_describes_live_listen_build_and_waiting_port() {
    let notification = notification(&live("abc123"));
    assert_eq!(notification.title, "claudex · 127.0.0.1:62789");
    assert_eq!(notification.subtitle, "live 更新完了 · build abc123");
    assert!(notification.body.contains("127.0.0.1:62789"));
    assert!(notification.body.contains("abc123"));
    assert!(notification.body.contains("127.0.0.1:8318"));
    assert!(notification.body.contains("即時利用"));
}

#[test]
fn swap_complete_notification_describes_build_and_listen() {
    let notification = notification(&complete("abc123"));
    assert_eq!(notification.title, "claudex · 127.0.0.1:8318");
    assert_eq!(notification.subtitle, "差し替え完了 · build abc123");
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
    assert!(args[1].contains("with title \"claudex · 127.0.0.1:8318\""));
    assert!(args[1].contains("subtitle \"差し替え完了 · build q\\\"w\""));
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
fn only_swap_complete_notifies_and_dedupes_same_build() {
    let first_complete = complete("abc");
    assert!(
        !should_emit(&waiting("abc", 1), None),
        "waiting must stay silent"
    );
    assert!(
        !should_emit(&live("abc"), None),
        "live ready must stay silent"
    );
    assert!(
        should_emit(&first_complete, None),
        "first swap complete must notify"
    );
    assert!(
        !should_emit(&first_complete, Some(&LastNotify::from(&first_complete))),
        "duplicate swap complete for the same build must not notify"
    );
    assert!(
        !should_emit(&waiting("def", 2), Some(&LastNotify::from(&first_complete))),
        "waiting for a newer build must stay silent"
    );
    assert!(
        should_emit(&complete("def"), Some(&LastNotify::from(&first_complete))),
        "a newer build complete with no cooldown timestamp may notify"
    );
}

#[test]
fn rapid_rebuilds_on_the_same_port_are_suppressed_until_cooldown() {
    let mut last = LastNotify::from(&complete("abc"));
    last.emitted_unix = 1_700_000_000;
    assert!(
        !should_emit_at(&waiting("def", 2), Some(&last), last.emitted_unix + 30),
        "waiting never notifies"
    );
    assert!(
        !should_emit_at(&complete("def"), Some(&last), last.emitted_unix + 60),
        "a newer build within 5 minutes must not spam swap complete"
    );
    assert!(
        !should_emit_at(
            &waiting("def", 2),
            Some(&last),
            last.emitted_unix + RAPID_REBUILD_COOLDOWN_SECS + 1
        ),
        "waiting stays silent even after the quiet gap"
    );
    assert!(
        should_emit_at(
            &complete("def"),
            Some(&last),
            last.emitted_unix + RAPID_REBUILD_COOLDOWN_SECS + 1
        ),
        "past the quiet gap a newer build complete may notify again"
    );
}

#[test]
fn waiting_and_live_never_notify_even_on_different_listen() {
    let previous = LastNotify::from(&complete("abc"));
    let moved = Event::WaitingForIdle {
        listen: "127.0.0.1:9999".to_owned(),
        build_id: "abc".to_owned(),
        waiter_pid: 2,
    };
    assert!(!should_emit_at(&moved, Some(&previous), previous.emitted_unix + 1));
    assert!(!should_emit_at(&live("abc"), Some(&previous), previous.emitted_unix + 1));
}

#[test]
fn same_build_waiting_live_complete_collapse_to_one_complete() {
    let mut last = LastNotify::from(&waiting("abc", 1));
    last.emitted_unix = 1_700_000_000;
    assert!(
        !should_emit_at(&live("abc"), Some(&last), last.emitted_unix + 5),
        "Waiting → Live must not notify"
    );
    assert!(
        should_emit_at(&complete("abc"), Some(&last), last.emitted_unix + 5),
        "Complete may notify once if only Waiting/Live was recorded"
    );
    last = LastNotify::from(&complete("abc"));
    last.emitted_unix = 1_700_000_010;
    assert!(
        !should_emit_at(&complete("abc"), Some(&last), last.emitted_unix + 5),
        "second Complete for the same build must stay quiet"
    );
}

#[test]
fn same_build_does_not_regress_from_complete_to_waiting() {
    let mut last = LastNotify::from(&complete("abc"));
    last.emitted_unix = 1_700_000_000;
    assert!(
        !should_emit_at(&waiting("abc", 2), Some(&last), last.emitted_unix + 5),
        "Complete → Waiting for the same build must not spam"
    );
    assert!(
        !should_emit_at(&live("abc"), Some(&last), last.emitted_unix + 5),
        "Complete → Live for the same build must not spam"
    );
}

#[test]
fn post_emits_only_one_complete_for_waiting_live_complete_flow() {
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
        vec![complete("abc")],
        "same build must notify swap complete once, never waiting/live"
    );
}

#[test]
fn post_emits_exactly_one_notification_for_four_same_build_completes() {
    let root = tempfile::tempdir().expect("notify cache");
    let listen = listen();
    let events = TestEvents::capture();
    for _ in 0..4 {
        post(root.path(), &listen, complete("same-build"));
    }
    let emitted = events.take();
    assert_eq!(
        emitted.len(),
        1,
        "four Completes for one build_id must emit exactly one notification (got {})",
        emitted.len()
    );
    assert_eq!(emitted, vec![complete("same-build")]);
}

#[test]
fn post_dedupes_same_build_across_different_listen_ports() {
    // cargo install can hot-swap several listeners; each used to notify once
    // via a per-listen dedup file → N banners with the same build_id.
    let root = tempfile::tempdir().expect("notify cache");
    let ports: [SocketAddr; 4] = [
        "127.0.0.1:8318".parse().expect("listen"),
        "127.0.0.1:53087".parse().expect("listen"),
        "127.0.0.1:59787".parse().expect("listen"),
        "127.0.0.1:62789".parse().expect("listen"),
    ];
    let events = TestEvents::capture();
    for addr in ports {
        post(root.path(), &addr, complete_on(addr, "shared-build"));
    }
    let emitted = events.take();
    assert_eq!(
        emitted.len(),
        1,
        "same build_id across {} listen ports must notify once (got {})",
        ports.len(),
        emitted.len()
    );
    assert_eq!(emitted, vec![complete_on(ports[0], "shared-build")]);
    assert_eq!(
        launcher_logs::hot_swap_notify_path(root.path(), &ports[0]),
        root.path().join("hot-swap-notify.json"),
        "dedup state must be shared for the whole cache, not per listen"
    );
    assert!(
        !root
            .path()
            .join("hot-swap-notify.127_0_0_1_8318.json")
            .exists(),
        "legacy per-listen notify files must not be written"
    );
}

#[test]
fn post_suppresses_rapid_rebuild_complete_and_slides_cooldown() {
    let root = tempfile::tempdir().expect("notify cache");
    let listen = listen();
    let events = TestEvents::capture();
    post(root.path(), &listen, complete("abc"));
    post(root.path(), &listen, complete("def"));
    post(root.path(), &listen, complete("ghi"));
    assert_eq!(
        events.take(),
        vec![complete("abc")],
        "rapid successive builds must notify swap complete only once"
    );
    let last = read_last(root.path(), &listen).expect("dedup state");
    assert_eq!(last.build_id, "abc");
    assert!(last.emitted_unix > 0, "suppressed attempts must slide cooldown");
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
    post(root.path(), &listen(), complete("dead"));
    assert_eq!(spawn.take_events(), vec![complete("dead")]);
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
    post(root.path(), &listen, complete("abc"));
    assert_eq!(events.take(), vec![complete("abc")]);
}
