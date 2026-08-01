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
        // The child lock is also the cleanup-completion barrier. Acquire it
        // before publishing `alive = false` so every later stop caller waits
        // for the cleanup owner to terminate and reap the provider.
        let mut child = self.child.lock().await;
        if !self.alive.swap(false, Ordering::Relaxed) {
            return;
        }
        self.fail_pending(reason).await;
        self.event_dispatcher.close();

        let process_group = child.id();
        let status = match child.try_wait() {
            Ok(Some(status)) => finish_completed_child(status, process_group).await,
            Ok(None) => terminate_child(&mut child, process_group).await,
            Err(error) => cleanup_after_inspection_error(&mut child, process_group, error).await,
        };
        // Drain requests that raced with the initial drain but were flushed
        // before process termination. Requests started after this point write
        // to a reaped child and remove their own pending entry on failure.
        self.fail_pending(reason).await;
        tracing::error!(?status, %reason, "codex app-server stopped");
    }
}

async fn finish_completed_child(
    status: std::process::ExitStatus,
    process_group: Option<u32>,
) -> std::io::Result<std::process::ExitStatus> {
    if let Some(process_group) = process_group {
        terminate_process_group(process_group).await;
    }
    Ok(status)
}

async fn cleanup_after_inspection_error(
    child: &mut tokio::process::Child,
    process_group: Option<u32>,
    error: std::io::Error,
) -> std::io::Result<std::process::ExitStatus> {
    tracing::error!(?error, "failed to inspect codex app-server process");
    if let Err(cleanup_error) = terminate_child(child, process_group).await {
        tracing::error!(
            ?cleanup_error,
            "failed to reap codex app-server after process inspection error"
        );
    }
    Err(error)
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
#[path = "lifecycle_tests.rs"]
mod tests;
