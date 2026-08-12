//! Bounded, cancellable JSONL writes for a persistent app-server connection.
//!
//! A request must reserve a queue slot before it publishes its pending response
//! state.  This ordering is important: a caller that is cancelled while the
//! provider's stdin is under backpressure must not leave a response entry that
//! can never be completed.  Reservations are represented by an owned mpsc
//! permit, so dropping a cancelled future returns the slot to the queue.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::{Mutex, Notify, mpsc, oneshot},
    task::JoinHandle,
};

/// Keep provider stdin bounded.  The app-server protocol is request/response
/// multiplexed, so a small queue preserves parallelism without allowing a
/// disconnected provider to retain an unbounded amount of memory.
const DEFAULT_QUEUE_CAPACITY: usize = 64;

/// Stop waits briefly for already-reserved frames to flush before it kills the
/// provider process group.  A provider that does not consume stdin must not
/// hold daemon shutdown indefinitely.
pub(super) const DRAIN_TIMEOUT: Duration = Duration::from_millis(250);

const OPEN: u8 = 0;
const DRAINING: u8 = 1;
const FAILED: u8 = 2;

struct WriterState {
    mode: AtomicU8,
    reserved: AtomicUsize,
    fatal: Mutex<Option<String>>,
    changed: Notify,
}

impl WriterState {
    fn new() -> Self {
        Self {
            mode: AtomicU8::new(OPEN),
            reserved: AtomicUsize::new(0),
            fatal: Mutex::new(None),
            changed: Notify::new(),
        }
    }

    fn release_reserved(&self) {
        // A failed writer drains queued frames and concurrently drops permits
        // owned by cancelled callers.  Saturating subtraction keeps that race
        // harmless while preserving the no-leak invariant.
        let _ = self
            .reserved
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_sub(1)
            });
        self.changed.notify_waiters();
    }

    async fn set_fatal(&self, reason: String) {
        let mut fatal = self.fatal.lock().await;
        if fatal.is_none() {
            *fatal = Some(reason);
        }
        drop(fatal);
        self.changed.notify_waiters();
    }

    fn mark_failed(&self) {
        self.mode.store(FAILED, Ordering::Release);
    }

    async fn failure(&self) -> Option<String> {
        self.fatal.lock().await.clone()
    }
}

struct Frame {
    bytes: Vec<u8>,
    state: Arc<WriterState>,
    completion: Option<oneshot::Sender<Result<(), String>>>,
    completed: bool,
}

impl Frame {
    fn complete(&mut self, result: Result<(), String>) {
        if self.completed {
            return;
        }
        self.completed = true;
        if let Some(completion) = self.completion.take() {
            let _ = completion.send(result);
        }
        self.state.release_reserved();
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        self.complete(Err("app-server writer frame was dropped".to_owned()));
    }
}

/// A queue reservation held by one protocol frame.  The reservation is
/// intentionally separate from the pending-response map: cancellation before
/// [`FrameReservation::send`] drops both the permit and the frame budget.
pub(super) struct FrameReservation {
    permit: Option<mpsc::OwnedPermit<Frame>>,
    state: Arc<WriterState>,
    committed: bool,
}

impl FrameReservation {
    pub(super) fn send(mut self, bytes: Vec<u8>) -> oneshot::Receiver<Result<(), String>> {
        self.committed = true;
        let (completion, receiver) = oneshot::channel();
        if let Some(permit) = self.permit.take() {
            permit.send(Frame {
                bytes,
                state: Arc::clone(&self.state),
                completion: Some(completion),
                completed: false,
            });
        }
        receiver
    }
}

impl Drop for FrameReservation {
    fn drop(&mut self) {
        if !self.committed {
            self.state.release_reserved();
        }
    }
}

/// The single writer task and its bounded queue.
pub(super) struct FrameWriter {
    sender: Mutex<Option<mpsc::Sender<Frame>>>,
    state: Arc<WriterState>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl FrameWriter {
    pub(super) fn spawn(stdin: tokio::process::ChildStdin) -> Arc<Self> {
        Self::spawn_with_capacity(stdin, DEFAULT_QUEUE_CAPACITY)
    }

    fn spawn_with_capacity<W>(writer: W, capacity: usize) -> Arc<Self>
    where
        W: AsyncWrite + Unpin + Send + 'static,
    {
        assert!(capacity > 0, "app-server writer queue must be non-empty");
        let state = Arc::new(WriterState::new());
        let (sender, receiver) = mpsc::channel(capacity);
        let task_state = Arc::clone(&state);
        let task = tokio::spawn(async move { run_writer(writer, receiver, task_state).await });
        Arc::new(Self {
            sender: Mutex::new(Some(sender)),
            state,
            task: Mutex::new(Some(task)),
        })
    }

    /// Reserve a bounded frame slot.  The future is cancellation-safe: no
    /// application state is touched until the owned queue permit is returned.
    pub(super) async fn reserve(&self) -> Result<FrameReservation> {
        if let Some(reason) = self.state.failure().await {
            bail!("app-server writer failed: {reason}");
        }
        if self.state.mode.load(Ordering::Acquire) != OPEN {
            return Err(self.closed_error());
        }
        let sender = self
            .sender
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| self.closed_error())?;
        let permit = sender
            .reserve_owned()
            .await
            .map_err(|_| self.closed_error())?;
        // Count the permit before the second state check.  If shutdown races
        // this point, shutdown observes it and waits (or times out) rather
        // than declaring the queue drained too early.
        self.state.reserved.fetch_add(1, Ordering::AcqRel);
        if self.state.mode.load(Ordering::Acquire) != OPEN {
            drop(permit);
            self.state.release_reserved();
            return Err(self.closed_error());
        }
        Ok(FrameReservation {
            permit: Some(permit),
            state: Arc::clone(&self.state),
            committed: false,
        })
    }

    pub(super) async fn begin_shutdown(&self) {
        let previous = self.state.mode.swap(DRAINING, Ordering::AcqRel);
        if previous != DRAINING {
            // Existing OwnedPermits keep the channel alive long enough for
            // their frames to be sent; no new reservation can be created.
            self.sender.lock().await.take();
        }
        self.state.changed.notify_waiters();
    }

    pub(super) async fn drain(&self, timeout: Duration) {
        self.begin_shutdown().await;
        let _ = tokio::time::timeout(timeout, wait_until_drained(Arc::clone(&self.state))).await;
    }

    pub(super) async fn join(&self) {
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }

    /// Watch without keeping the writer alive.  This is used by AppServer's
    /// fatal-error monitor so dropping an otherwise idle AppServer also drops
    /// its stdin task instead of leaving the monitor as an owner.
    pub(super) async fn wait_for_fatal_weak(writer: std::sync::Weak<Self>) -> Option<String> {
        while let Some(writer) = writer.upgrade() {
            let reason = writer.state.failure().await;
            drop(writer);
            match reason {
                Some(reason) => return Some(reason),
                None => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
        None
    }

    fn closed_error(&self) -> anyhow::Error {
        anyhow!("app-server writer is closed")
    }
}

async fn run_writer<W>(mut writer: W, mut receiver: mpsc::Receiver<Frame>, state: Arc<WriterState>)
where
    W: AsyncWrite + Unpin,
{
    while let Some(mut frame) = receiver.recv().await {
        let result = match writer.write_all(&frame.bytes).await {
            Ok(()) => writer.flush().await,
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            let reason = format!("failed to write codex app-server input: {error}");
            fail_writer(frame, &mut receiver, &state, reason).await;
            return;
        }
        frame.complete(Ok(()));
    }
    state.changed.notify_waiters();
}

async fn wait_until_drained(state: Arc<WriterState>) {
    while state.reserved.load(Ordering::Acquire) != 0
        && state.mode.load(Ordering::Acquire) != FAILED
    {
        state.changed.notified().await;
    }
}

async fn fail_writer(
    mut frame: Frame,
    receiver: &mut mpsc::Receiver<Frame>,
    state: &Arc<WriterState>,
    reason: String,
) {
    // Fence reservations before draining queued frames.  A caller racing
    // this failure must never enqueue a frame after the drain pass, otherwise
    // its completion channel could be orphaned.
    state.mark_failed();
    frame.complete(Err(reason.clone()));
    // Drop queued frames so each one releases its reservation.  A caller
    // waiting in reserve() observes the fatal state and exits.
    while let Ok(mut queued) = receiver.try_recv() {
        queued.complete(Err(reason.clone()));
    }
    state.set_fatal(reason).await;
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    include!("writer_tests.rs");
}
