use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use super::*;
use tokio::sync::{Barrier, Notify};

#[cfg(unix)]
#[tokio::test]
async fn stops_a_completed_parent_and_its_live_process_group() {
    let root = tempfile::tempdir().expect("lifecycle fixture");
    let source = source_home(root.path());
    let program = script(
        root.path(),
        "completed-parent",
        "read initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nsleep 30 &\n",
    );
    let server = AppServer::spawn_with_program(
        "model",
        &program,
        &source,
        &root.path().join("completed-parent-home"),
    )
    .await
    .expect("start app-server fixture");
    let process_group = server
        .child
        .lock()
        .await
        .id()
        .expect("app-server process group");

    tokio::time::sleep(Duration::from_millis(20)).await;
    server.stop("completed parent lifecycle test").await;

    assert!(
        server
            .child
            .lock()
            .await
            .try_wait()
            .expect("inspect completed parent")
            .is_some()
    );
    assert!(!process_group_is_alive(&format!("-{process_group}")));
}

#[cfg(unix)]
#[tokio::test]
async fn stop_if_alive_stops_the_upgraded_server() {
    let root = tempfile::tempdir().expect("lifecycle fixture");
    let source = source_home(root.path());
    let program = script(
        root.path(),
        "live-server",
        "read initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nwhile read line; do :; done\n",
    );
    let server = AppServer::spawn_with_program(
        "model",
        &program,
        &source,
        &root.path().join("live-server-home"),
    )
    .await
    .expect("start app-server fixture");
    let weak = Arc::downgrade(&server);

    stop_if_alive(&weak, "weak lifecycle test").await;

    assert!(!server.is_alive());
    assert!(
        server
            .child
            .lock()
            .await
            .try_wait()
            .expect("inspect stopped child")
            .is_some()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn stop_cleans_up_when_another_waiter_has_reaped_the_parent() {
    let root = tempfile::tempdir().expect("lifecycle fixture");
    let source = source_home(root.path());
    let program = script(
        root.path(),
        "externally-reaped-parent",
        "read initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nsleep 30 &\n",
    );
    let server = AppServer::spawn_with_program(
        "model",
        &program,
        &source,
        &root.path().join("externally-reaped-parent-home"),
    )
    .await
    .expect("start app-server fixture");
    let process_group = server
        .child
        .lock()
        .await
        .id()
        .expect("app-server process group");
    let mut exit_status = 0;
    let waited = unsafe { libc::waitpid(process_group as i32, &mut exit_status, 0) };
    assert_eq!(waited, process_group as i32);

    server.stop("externally reaped lifecycle test").await;

    assert!(!server.is_alive());
    assert!(!process_group_is_alive(&format!("-{process_group}")));
}

#[cfg(unix)]
#[tokio::test]
async fn concurrent_stop_waits_for_the_cleanup_owner() {
    let root = tempfile::tempdir().expect("concurrent lifecycle fixture");
    let source = source_home(root.path());
    let ready = root.path().join("term-handler-ready");
    let program = script(
        root.path(),
        "concurrent-stop",
        &format!(
            "read initialize\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\nread initialized\ntrap '' TERM\n: > '{}'\nwhile :; do sleep 1; done\n",
            ready.display()
        ),
    );
    let server = AppServer::spawn_with_program(
        "model",
        &program,
        &source,
        &root.path().join("concurrent-stop-home"),
    )
    .await
    .expect("start concurrent app-server fixture");
    wait_for_path(&ready).await;

    let start = Arc::new(Barrier::new(2));
    let first = spawn_barrier_stop(
        Arc::clone(&server),
        Arc::clone(&start),
        "concurrent cleanup owner",
    );
    start.wait().await;
    tokio::time::timeout(Duration::from_millis(100), wait_until_stopped(&server))
        .await
        .expect("cleanup owner marked the provider stopped");

    let second_finished = Arc::new(Notify::new());
    let second = spawn_notifying_stop(
        Arc::clone(&server),
        Arc::clone(&second_finished),
        "concurrent cleanup joiner",
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), second_finished.notified())
            .await
            .is_err(),
        "second stop returned while the cleanup owner was in its TERM grace period"
    );

    first.await.expect("cleanup owner task");
    tokio::time::timeout(Duration::from_secs(1), second_finished.notified())
        .await
        .expect("second stop joined completed cleanup");
    second.await.expect("cleanup joiner task");
    assert!(
        server
            .child
            .lock()
            .await
            .try_wait()
            .expect("inspect concurrently stopped child")
            .is_some(),
        "concurrent stop must return only after the direct child is reaped"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn terminate_child_accepts_a_missing_process_group() {
    let mut child = tokio::process::Command::new("sh")
        .args(["-c", "while :; do sleep 1; done"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start child");

    let status = terminate_child(&mut child, None)
        .await
        .expect("terminate child without a process group");

    assert!(!status.success());
}

#[cfg(unix)]
#[tokio::test]
async fn terminate_process_group_escalates_for_a_term_resistant_group() {
    let root = tempfile::tempdir().expect("term-resistant group fixture");
    let ready = root.path().join("term-handler-ready");
    let mut command = tokio::process::Command::new("sh");
    command
        .args([
            "-c",
            &format!(
                "trap '' TERM\n: > '{}'\nwhile :; do sleep 1; done",
                ready.display()
            ),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0);
    let mut child = command.spawn().expect("start term-resistant group");
    let process_group = child.id().expect("process group ID");
    wait_for_path(&ready).await;

    terminate_process_group(process_group).await;
    let status = child.wait().await.expect("reap term-resistant group");

    assert!(!status.success());
    assert!(!process_group_is_alive(&format!("-{process_group}")));
}

#[cfg(unix)]
fn script(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).expect("write script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("make script executable");
    path
}

#[cfg(unix)]
fn source_home(root: &Path) -> PathBuf {
    let source = root.join("source");
    std::fs::create_dir(&source).expect("create source home");
    std::fs::write(source.join("auth.json"), "{}").expect("write auth");
    source
}

#[cfg(unix)]
async fn wait_for_path(path: &Path) {
    tokio::time::timeout(Duration::from_millis(500), wait_until_path_exists(path))
        .await
        .expect("fixture installed its TERM handler");
}

fn spawn_barrier_stop(
    server: Arc<AppServer>,
    start: Arc<Barrier>,
    reason: &'static str,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        start.wait().await;
        server.stop(reason).await;
    })
}

fn spawn_notifying_stop(
    server: Arc<AppServer>,
    finished: Arc<Notify>,
    reason: &'static str,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        server.stop(reason).await;
        finished.notify_one();
    })
}

async fn wait_until_stopped(server: &AppServer) {
    while server.is_alive() {
        tokio::task::yield_now().await;
    }
}

async fn wait_until_path_exists(path: &Path) {
    while !path.exists() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
