use super::*;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::{
    io::{AsyncRead, AsyncWriteExt as _, ReadBuf},
    task::LocalSet,
};

struct FailingStderr;

impl AsyncRead for FailingStderr {
    fn poll_read(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::other("synthetic stderr read failure")))
    }
}

#[tokio::test]
async fn quota_line_becomes_terminal_without_waiting_for_eof_silence() {
    let (client, server) = tokio::io::duplex(256);
    let (tx, rx) = watch_channel();
    let mut client = client;
    tokio::spawn(async move {
        client
            .write_all(b"info: starting\nWeekly usage limit reached. Resets in 4 days.\n")
            .await
            .expect("write quota");
    });
    drain_quota_stderr(server, tx).await;
    assert_eq!(
        rx.borrow().as_deref(),
        Some("Weekly usage limit reached. Resets in 4 days.")
    );
}

#[tokio::test]
async fn non_quota_stderr_does_not_trip_the_watch() {
    let (client, server) = tokio::io::duplex(64);
    let (tx, rx) = watch_channel();
    let mut client = client;
    tokio::spawn(async move {
        client.write_all(b"ready\n").await.expect("write");
        drop(client);
    });
    drain_quota_stderr(server, tx).await;
    assert!(rx.borrow().is_none());
}

#[tokio::test]
async fn stderr_read_errors_leave_the_quota_watch_empty() {
    let (tx, rx) = watch_channel();
    drain_quota_stderr(FailingStderr, tx).await;
    assert!(rx.borrow().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn spawned_stderr_watcher_forwards_quota_from_a_provider_process() {
    LocalSet::new()
        .run_until(async {
            let mut child = tokio::process::Command::new("sh")
                .args(["-c", "printf 'Monthly usage limit reached\\n' >&2"])
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("spawn fixture provider");
            let (tx, mut rx) = watch_channel();
            spawn_watch(child.stderr.take().expect("piped stderr"), tx);
            child.wait().await.expect("fixture provider exits");
            assert_eq!(
                tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    wait_quota_message(Some(&mut rx))
                )
                .await
                .expect("quota notification")
                .as_deref(),
                Some("Monthly usage limit reached")
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn wait_quota_message_receives_a_value_sent_after_waiting() {
    let (tx, mut rx) = watch_channel();
    let waiter = tokio::spawn(async move { wait_quota_message(Some(&mut rx)).await });
    tokio::task::yield_now().await;
    tx.send(Some("Monthly usage limit reached".to_owned()))
        .expect("send");
    assert_eq!(
        waiter.await.expect("waiter").as_deref(),
        Some("Monthly usage limit reached")
    );
}

#[tokio::test]
async fn quota_wait_ignores_an_empty_update_before_the_quota_message() {
    let (tx, mut rx) = watch_channel();
    let waiter = tokio::spawn(async move { wait_quota_message(Some(&mut rx)).await });
    tokio::task::yield_now().await;
    tx.send(None).expect("empty update");
    // Let the waiter observe the versioned empty update before replacing it.
    tokio::task::yield_now().await;
    tx.send(Some("Daily usage limit reached".to_owned()))
        .expect("quota update");
    assert_eq!(
        waiter.await.expect("waiter").as_deref(),
        Some("Daily usage limit reached")
    );
}

#[tokio::test]
async fn quota_wait_without_a_receiver_stays_pending() {
    let mut wait = std::pin::pin!(wait_quota_message(None));
    std::future::poll_fn(|context| {
        assert!(std::future::Future::poll(wait.as_mut(), context).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
}
