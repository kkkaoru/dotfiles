#![cfg(unix)]
#![cfg_attr(coverage_nightly, coverage(off))]

use super::descriptors::detach_session_and_close_with;
use super::{
    StartedDaemon, bounded_descriptor_limit, close_file_descriptor,
    close_inherited_descriptors_with, close_system, configure_process_group,
    terminate_started_recovery,
};

#[cfg(unix)]
fn wait_until_exited(child: &mut std::process::Child, attempts: usize) -> bool {
    for _ in 0..attempts {
        match child.try_wait().expect("poll recovery child") {
            Some(_) => return true,
            None => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
    false
}

#[cfg(unix)]
fn wait_for_ready_file(
    path: &std::path::Path,
    attempts: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..attempts {
        if std::fs::read_to_string(path).is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Err("session fixture was not ready".into())
}

#[cfg(unix)]
fn validate_detached_session(
    path: &std::path::Path,
    child_pid: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    wait_for_ready_file(path, 40)?;
    let pid = child_pid as libc::pid_t;
    let session_id = unsafe { libc::getsid(pid) };
    let process_group_id = unsafe { libc::getpgid(pid) };
    assert_eq!(session_id, pid, "session id must be daemon pid");
    assert_eq!(process_group_id, pid, "process group must be daemon pid");
    Ok(())
}

#[cfg(unix)]
use std::{
    fs,
    process::{Command, Stdio},
};

#[cfg(unix)]
#[test]
fn terminate_started_recovery_stops_a_detached_child() {
    let mut command = Command::new("sleep");
    command
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_process_group(&mut command);
    let mut child = command.spawn().expect("spawn recovery terminate fixture");
    let pid = child.id();
    terminate_started_recovery(pid);
    if wait_until_exited(&mut child, 50) {
        return;
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("terminate_started_recovery must stop pid {pid}");
}

#[test]
fn bounds_descriptor_limits_before_closing_inherited_fds() {
    assert_eq!(bounded_descriptor_limit(4), 4);
    assert_eq!(bounded_descriptor_limit(3), 1024);
    assert_eq!(bounded_descriptor_limit(1_048_576), 1024);
}

#[test]
fn closes_the_inherited_descriptor_range_via_the_injected_operation() {
    let mut closed = Vec::new();
    close_inherited_descriptors_with(6, |fd| closed.push(fd))
        .expect("close operation is infallible");
    assert_eq!(closed, vec![3, 4, 5]);
    close_system(|_| {}).expect("system descriptor limit is available");
    close_file_descriptor(-1);
}

#[test]
fn detaches_before_closing_descriptors_with_injected_operations() {
    let events = std::cell::RefCell::new(Vec::new());
    detach_session_and_close_with(
        || {
            events.borrow_mut().push("detach");
            Ok(())
        },
        || {
            events.borrow_mut().push("close");
            Ok(())
        },
    )
    .expect("detach and close");
    assert_eq!(events.into_inner(), ["detach", "close"]);

    let close_called = std::cell::Cell::new(false);
    let error = detach_session_and_close_with(
        || Err(std::io::Error::other("detach failed")),
        || {
            close_called.set(true);
            Ok(())
        },
    )
    .expect_err("detach error");
    assert_eq!(error.kind(), std::io::ErrorKind::Other);
    assert!(!close_called.get());

    let error =
        detach_session_and_close_with(|| Ok(()), || Err(std::io::Error::other("close failed")))
            .expect_err("close error");
    assert_eq!(error.kind(), std::io::ErrorKind::Other);
}

#[test]
fn started_daemon_disarms_only_when_ownership_is_transferred() {
    let terminated = std::cell::Cell::new(0);
    let started = StartedDaemon::with_terminate(42, |pid| terminated.set(pid));
    assert_eq!(started.pid(), 42);
    assert_eq!(started.disarm(), 42);
    assert_eq!(terminated.get(), 0);
}

#[tokio::test]
async fn started_daemon_terminates_when_a_post_spawn_future_is_cancelled() {
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };

    let terminated = Arc::new(AtomicU32::new(0));
    let observed = Arc::clone(&terminated);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let terminate = move |pid| observed.store(pid, Ordering::SeqCst);
        let _started = StartedDaemon::with_terminate(77, terminate);
        let _ = ready_tx.send(());
        std::future::pending::<()>().await;
    });
    ready_rx.await.expect("guarded future started");
    task.abort();
    let _ = task.await;
    assert_eq!(terminated.load(Ordering::SeqCst), 77);
}

#[cfg(unix)]
#[test]
fn daemon_starts_in_its_own_session_and_process_group() {
    let path =
        std::env::temp_dir().join(format!("claudex-daemon-session-{}.txt", std::process::id()));
    let _ = fs::remove_file(&path);

    let script = r#"
        printf 'ready\n' > "$1"
        sleep 30
    "#;
    let mut command = Command::new("sh");
    command
        .args(["-c", script, "claudex-session-test", path.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_process_group(&mut command);
    let mut child = command.spawn().expect("spawn detached session fixture");

    let validation = validate_detached_session(&path, child.id());

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(&path);
    validation.expect("daemon must be detached into a new session");
}
