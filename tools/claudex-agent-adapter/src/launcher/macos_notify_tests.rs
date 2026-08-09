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
    post(root.path(), &listen, complete("abc"));
    post(root.path(), &listen, complete("abc"));
    assert_eq!(
        events.take(),
        vec![waiting("abc", 9), complete("abc")],
        "old failure notified every arm/replace; new dedup keeps one waiting and one complete"
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
