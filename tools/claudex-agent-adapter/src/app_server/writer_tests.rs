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
        if self.released.load(Ordering::Relaxed) != 0 {
            self.writes.fetch_add(1, Ordering::Relaxed);
            return std::task::Poll::Ready(Ok(bytes.len()));
        }
        cx.waker().wake_by_ref();
        std::task::Poll::Pending
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
    let _ = first.send(b"first\n".to_vec());
    let blocked = tokio::spawn({
        let queue = Arc::clone(&queue);
        async move { queue.reserve().await }
    });
    tokio::task::yield_now().await;
    blocked.abort();
    let _ = blocked.await;
    released.store(1, Ordering::Relaxed);
    tokio::time::timeout(Duration::from_secs(1), async {
        while writes.load(Ordering::Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first frame was written");
    let second = queue
        .reserve()
        .await
        .expect("cancelled reservation released");
    let _ = second.send(b"second\n".to_vec());
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
    let reason = tokio::time::timeout(Duration::from_secs(1), queue.wait_for_fatal())
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
    let _ = frame.send(vec![b'x'; 1024]);
    queue.drain(Duration::from_millis(1)).await;
    // The writer task is intentionally still blocked on the full duplex
    // stream; shutdown's timeout is the bounded contract.
    drop(reader);
    queue.join().await;
}
