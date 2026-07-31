use std::sync::{Weak, atomic::Ordering};

#[cfg(unix)]
use std::{
    process::{Command, Stdio},
    time::Duration,
};

#[cfg(unix)]
use tokio::time::Instant;

#[cfg(unix)]
const STOP_GRACE_PERIOD: Duration = Duration::from_millis(250);

use super::AppServer;

impl AppServer {
    pub(super) async fn stop(&self, reason: &str) {
        if !self.alive.swap(false, Ordering::Relaxed) {
            return;
        }
        self.fail_pending(reason).await;
        self.event_dispatcher.close();

        let mut child = self.child.lock().await;
        let process_group = child.id();
        let status = match child.try_wait() {
            Ok(Some(status)) => {
                if let Some(process_group) = process_group {
                    terminate_process_group(process_group).await;
                }
                Ok(status)
            }
            Ok(None) => terminate_child(&mut child, process_group).await,
            Err(error) => {
                tracing::error!(?error, "failed to inspect codex app-server process");
                if let Err(cleanup_error) = terminate_child(&mut child, process_group).await {
                    tracing::error!(
                        ?cleanup_error,
                        "failed to reap codex app-server after process inspection error"
                    );
                }
                Err(error)
            }
        };
        tracing::error!(?status, %reason, "codex app-server stopped");
    }
}

async fn terminate_child(
    child: &mut tokio::process::Child,
    process_group: Option<u32>,
) -> std::io::Result<std::process::ExitStatus> {
    if let Some(process_group) = process_group {
        terminate_process_group(process_group).await;
    }
    let _ = child.start_kill();
    child.wait().await
}

pub(super) async fn stop_if_alive(server: &Weak<AppServer>, reason: &str) {
    if let Some(server) = server.upgrade() {
        server.stop(reason).await;
    }
}

#[cfg(unix)]
async fn terminate_process_group(process_group: u32) {
    let process_group = format!("-{process_group}");
    let _status = Command::new("kill")
        .args(["-TERM", &process_group])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let deadline = Instant::now() + STOP_GRACE_PERIOD;
    while process_group_is_alive(&process_group) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let _status = Command::new("kill")
        .args(["-KILL", &process_group])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
async fn terminate_process_group(_process_group: u32) {}

#[cfg(unix)]
fn process_group_is_alive(process_group: &str) -> bool {
    Command::new("kill")
        .args(["-0", process_group])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::Arc,
        time::Duration,
    };

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

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
        tokio::time::timeout(Duration::from_millis(500), async {
            while !path.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("fixture installed its TERM handler");
    }
}
