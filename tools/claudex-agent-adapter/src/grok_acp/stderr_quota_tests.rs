use super::*;
use tokio::io::AsyncWriteExt as _;

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
async fn wait_quota_message_returns_an_already_set_value() {
    let (tx, mut rx) = watch_channel();
    tx.send(Some("Monthly usage limit reached".to_owned()))
        .expect("send");
    assert_eq!(
        wait_quota_message(Some(&mut rx)).await.as_deref(),
        Some("Monthly usage limit reached")
    );
}
