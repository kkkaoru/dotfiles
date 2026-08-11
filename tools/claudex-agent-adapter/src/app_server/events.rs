use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::AtomicU64,
    },
};

use serde_json::Value;
#[cfg(test)]
use tokio::sync::Notify;

use self::event_shape::{event_thread_id, is_bridge_event, is_terminal_event};

mod encoding;
mod event_shape;
mod queue_state;
mod unsubscribe;
#[cfg(test)]
use encoding::{encoded_string_content_bytes, event_bytes};
use unsubscribe::unsubscribe;

pub(super) const MAX_QUEUED_EVENTS: usize = 256;
pub(super) const MAX_QUEUED_BYTES: usize = 1024 * 1024;
/// Cap coalesced text-delta size so Claude Code paints provider progress in
/// frequent chunks instead of one stalled mega-frame when the consumer lags.
pub(super) const MAX_COALESCED_DELTA_CHARS: usize = 96;

type Subscribers = Vec<(u64, Arc<EventQueue>)>;
pub(super) type Registry = HashMap<String, ThreadRoute>;

#[derive(Default)]
pub(super) struct ThreadRoute {
    subscribers: Subscribers,
    backlog: QueueState,
}

#[derive(Default)]
pub(super) struct QueueState {
    pub(super) events: VecDeque<QueuedEvent>,
    pub(super) queued_bytes: usize,
    pub(super) closed: bool,
    pub(super) overflowed: bool,
    pub(super) terminal_seen: bool,
}

pub(super) struct QueuedEvent {
    pub(super) value: Value,
    pub(super) bytes: usize,
    pub(super) requeueable: bool,
}

#[path = "events_queue.rs"]
mod queue;
use queue::EventQueue;
#[cfg(test)]
use queue::QueuePoll;

#[derive(Default)]
pub(crate) struct ThreadEventDispatcher {
    channels: Arc<Mutex<Registry>>,
    next_id: AtomicU64,
}

#[path = "events_dispatch.rs"]
mod dispatch;

/// A receiver for notifications belonging to exactly one app-server thread.
pub struct ThreadEvents {
    thread_id: String,
    channel_id: u64,
    queue: Arc<EventQueue>,
    channels: Arc<Mutex<Registry>>,
}

impl ThreadEvents {
    /// Create an already-closed receiver for a provider route that was retired
    /// while a concurrent caller was attaching to its thread. Callers can
    /// handle this as a normal stream-closed outcome instead of bringing down
    /// the adapter with a panic.
    pub(crate) fn closed(thread_id: impl Into<String>) -> Self {
        let dispatcher = ThreadEventDispatcher::default();
        let events = dispatcher.subscribe(&thread_id.into());
        events.queue.close();
        events
    }

    pub async fn recv(&self) -> Option<Value> {
        self.queue.recv().await
    }
}

impl Drop for ThreadEvents {
    fn drop(&mut self) {
        unsubscribe(
            &mut self
                .channels
                .lock()
                .expect("thread event registry poisoned"),
            &self.thread_id,
            self.channel_id,
            &self.queue,
        );
        self.queue.close();
    }
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
