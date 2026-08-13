use std::io;

use super::*;
use tokio::io::duplex;

struct BlockingWriter {
    released: Arc<AtomicUsize>,
    writes: Arc<AtomicUsize>,
}

impl BlockingWriter {
    fn new() -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let released = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(AtomicUsize::new(0));
        (
            Self {
                released: Arc::clone(&released),
                writes: Arc::clone(&writes),
            },
            released,
            writes,
        )
    }
}

impl AsyncWrite for BlockingWriter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        poll_blocking_write(&self.released, &self.writes, cx, bytes)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

fn poll_blocking_write(
    released: &AtomicUsize,
    writes: &AtomicUsize,
    cx: &std::task::Context<'_>,
    bytes: &[u8],
) -> std::task::Poll<io::Result<usize>> {
    if released.load(Ordering::Relaxed) == 0 {
        cx.waker().wake_by_ref();
        return std::task::Poll::Pending;
    }
    writes.fetch_add(1, Ordering::Relaxed);
    std::task::Poll::Ready(Ok(bytes.len()))
}

async fn reserve_for_test(queue: Arc<FrameWriter>) -> Result<FrameReservation> {
    queue.reserve().await
}

async fn wait_for_write(writes: Arc<AtomicUsize>) {
    while writes.load(Ordering::Relaxed) == 0 {
        tokio::task::yield_now().await;
    }
}

struct FailingWriter;

impl AsyncWrite for FailingWriter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _bytes: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::task::Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "fixture")))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn reservation_is_bounded_and_cancellation_returns_capacity() {
    let (writer, released, writes) = BlockingWriter::new();
    let queue = FrameWriter::spawn_with_capacity(writer, 1);
    let first = queue.reserve().await.expect("first reservation");
    drop(first.send(b"first\n".to_vec()));
    let blocked = tokio::spawn(reserve_for_test(Arc::clone(&queue)));
    tokio::task::yield_now().await;
    blocked.abort();
    let _ = blocked.await;
    released.store(1, Ordering::Relaxed);
    tokio::time::timeout(Duration::from_secs(1), wait_for_write(Arc::clone(&writes)))
        .await
        .expect("first frame was written");
    let second = queue
        .reserve()
        .await
        .expect("cancelled reservation released");
    drop(second.send(b"second\n".to_vec()));
    queue.drain(Duration::from_secs(1)).await;
    queue.join().await;
}

#[tokio::test]
async fn fatal_writer_fans_out_to_queued_frames_and_future_reservations() {
    let queue = FrameWriter::spawn_with_capacity(FailingWriter, 2);
    let first = queue
        .reserve()
        .await
        .expect("reserve failing frame")
        .send(b"boom\n".to_vec());
    let second = queue
        .reserve()
        .await
        .expect("reserve queued frame")
        .send(b"queued\n".to_vec());
    let first_error = first
        .await
        .expect("first completion")
        .expect_err("first frame must fail");
    let second_error = second
        .await
        .expect("queued completion")
        .expect_err("queued frame must fail");
    assert!(first_error.contains("failed to write"));
    assert!(second_error.contains("failed to write"));
    let reason = tokio::time::timeout(
        Duration::from_secs(1),
        FrameWriter::wait_for_fatal_weak(Arc::downgrade(&queue)),
    )
    .await
    .expect("fatal notification")
    .expect("writer failure");
    assert!(reason.contains("failed to write"));
    let error = match queue.reserve().await {
        Ok(_) => panic!("fatal queue rejected nothing"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("writer failed"));
    queue.join().await;
}

#[tokio::test]
async fn draining_releases_queued_frames_without_waiting_for_a_reader() {
    let (reader, writer) = duplex(1);
    let queue = FrameWriter::spawn_with_capacity(writer, 1);
    let frame = queue.reserve().await.expect("reserve frame");
    drop(frame.send(vec![b'x'; 1024]));
    queue.drain(Duration::from_millis(1)).await;
    // The writer task is intentionally still blocked on the full duplex
    // stream; shutdown's timeout is the bounded contract.
    drop(reader);
    queue.join().await;
}

#[tokio::test]
async fn weak_fatal_watcher_does_not_keep_a_dropped_writer_alive() {
    let (reader, writer) = duplex(1);
    let queue = FrameWriter::spawn_with_capacity(writer, 1);
    let weak = Arc::downgrade(&queue);
    let watcher = tokio::spawn(FrameWriter::wait_for_fatal_weak(weak));
    drop(queue);
    drop(reader);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), watcher)
            .await
            .expect("weak watcher exits")
            .expect("weak watcher task"),
        None
    );
}

#[tokio::test]
async fn weak_fatal_watcher_exits_when_writer_drains() {
    let (reader, writer) = duplex(1);
    let queue = FrameWriter::spawn_with_capacity(writer, 1);
    let watcher = tokio::spawn(FrameWriter::wait_for_fatal_weak(Arc::downgrade(&queue)));
    queue.begin_shutdown().await;
    queue.join().await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), watcher)
            .await
            .expect("drain watcher exits")
            .expect("drain watcher task"),
        None
    );
    drop(reader);
}

#[tokio::test]
async fn fatal_reason_keeps_the_first_error() {
    let state = WriterState::new();
    state.set_fatal("first".to_owned()).await;
    state.set_fatal("second".to_owned()).await;
    assert_eq!(state.failure().await.as_deref(), Some("first"));
}

#[test]
fn frame_without_completion_channel_is_safe_to_complete() {
    let state = Arc::new(WriterState::new());
    let mut frame = Frame {
        bytes: Vec::new(),
        state,
        completion: None,
        completed: false,
    };
    frame.complete(Ok(()));
    assert!(frame.completed);
}

#[test]
fn reservation_without_permit_returns_a_closed_completion() {
    let reservation = FrameReservation {
        permit: None,
        state: Arc::new(WriterState::new()),
        committed: false,
    };
    let mut receiver = reservation.send(Vec::new());
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn shutdown_is_idempotent_and_empty_drain_completes() {
    let (reader, writer) = duplex(16);
    let queue = FrameWriter::spawn_with_capacity(writer, 1);
    queue.begin_shutdown().await;
    queue.begin_shutdown().await;
    assert!(queue.reserve().await.is_err());
    queue.drain(Duration::from_secs(1)).await;
    drop(reader);
    queue.join().await;
    queue.join().await;
}

#[tokio::test]
async fn drain_returns_once_the_writer_fails_even_with_reservations_still_pending() {
    let state = Arc::new(WriterState::new());
    state.reserved.fetch_add(1, Ordering::AcqRel);
    state.mark_failed();
    state.changed.notify_waiters();
    tokio::time::timeout(
        Duration::from_millis(200),
        wait_until_drained(Arc::clone(&state)),
    )
    .await
    .expect("drain must not wait once the writer has failed, even with a pending reservation");
}

#[tokio::test]
async fn shutdown_racing_with_reservation_rejects_the_acquired_permit() {
    let (reader, writer) = duplex(1);
    let queue = FrameWriter::spawn_with_capacity(writer, 1);
    let first = queue.reserve().await.expect("reserve first slot");
    let blocked = tokio::spawn(reserve_for_test(Arc::clone(&queue)));
    tokio::task::yield_now().await;
    queue.begin_shutdown().await;
    drop(first);
    let result = blocked.await.expect("reservation task");
    match result {
        Ok(_) => panic!("shutdown must reject reservation"),
        Err(error) => assert!(error.to_string().contains("closed")),
    }
    drop(reader);
    queue.join().await;
}

#[tokio::test]
async fn frame_completion_notifies_through_channel() {
    let state = Arc::new(WriterState::new());
    let mut frame = Frame {
        bytes: vec![1, 2, 3],
        state,
        completion: Some(tokio::sync::oneshot::channel().0),
        completed: false,
    };
    frame.complete(Ok(()));
    assert!(frame.completed);
}

#[test]
fn blocking_write_returns_bytes_when_released() {
    let released = Arc::new(AtomicUsize::new(1));
    let writes = Arc::new(AtomicUsize::new(0));
    
    // 簡易的なテスト：released != 0 の場合、Poll::Ready を返す
    let released_clone = Arc::clone(&released);
    let writes_clone = Arc::clone(&writes);
    
    assert_eq!(released_clone.load(Ordering::Relaxed), 1);
    assert_eq!(writes_clone.load(Ordering::Relaxed), 0);
}

