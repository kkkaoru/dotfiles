use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::ChildStderr,
    sync::watch,
};

use crate::anthropic::contains_opencode_quota_marker;

pub(super) fn watch_channel() -> (
    watch::Sender<Option<String>>,
    watch::Receiver<Option<String>>,
) {
    watch::channel(None)
}

pub(super) fn spawn_watch(stderr: ChildStderr, quota: watch::Sender<Option<String>>) {
    tokio::task::spawn_local(drain_quota_stderr(stderr, quota));
}

pub(super) async fn drain_quota_stderr<R>(stderr: R, quota: watch::Sender<Option<String>>)
where
    R: AsyncRead + Unpin,
{
    let mut lines = BufReader::new(stderr).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => inspect_stderr_line(&quota, &line),
            Ok(None) => break,
            Err(error) => {
                tracing::debug!(?error, "OpenCode stderr closed");
                break;
            }
        }
    }
}

fn inspect_stderr_line(quota: &watch::Sender<Option<String>>, line: &str) {
    tracing::info!(target: "opencode_stderr", "{line}");
    if !contains_opencode_quota_marker(line) {
        return;
    }
    let _ = quota.send(Some(line.to_owned()));
}

pub(super) async fn wait_quota_message(
    quota: Option<&mut watch::Receiver<Option<String>>>,
) -> Option<String> {
    let Some(rx) = quota else {
        return std::future::pending().await;
    };
    if let Some(message) = rx.borrow().clone() {
        return Some(message);
    }
    loop {
        rx.changed().await.ok()?;
        if let Some(message) = rx.borrow().clone() {
            return Some(message);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "stderr_quota_tests.rs"]
mod tests;
