use std::{
    collections::{HashMap, VecDeque},
    mem,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde_json::Value;
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

impl ThreadEventDispatcher {
    pub(crate) fn subscribe(&self, thread_id: &str) -> ThreadEvents {
        let channel_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut channels = self
            .channels
            .lock()
            .expect("thread event registry poisoned");
        let route = channels.entry(thread_id.to_owned()).or_default();
        let queue = Arc::new(EventQueue {
            state: Mutex::new(mem::take(&mut route.backlog)),
            ready: Notify::new(),
        });
        route.subscribers.push((channel_id, Arc::clone(&queue)));
        drop(channels);
        ThreadEvents {
            thread_id: thread_id.to_owned(),
            channel_id,
            queue,
            channels: Arc::clone(&self.channels),
        }
    }

    pub(crate) fn dispatch(&self, event: Value) {
        if !is_bridge_event(&event) {
            return;
        }
        let Some(thread_id) = event_thread_id(&event) else {
            tracing::debug!(?event, "ignored app-server event without thread id");
            return;
        };
        let mut channels = self
            .channels
            .lock()
            .expect("thread event registry poisoned");
        if is_terminal_event(&event)
            && channels
                .get(thread_id)
                .is_none_or(|route| route.subscribers.is_empty())
        {
            channels.remove(thread_id);
            return;
        }
        let route = channels.entry(thread_id.to_owned()).or_default();
        if route.subscribers.is_empty() {
            route.backlog.push_or_overflow(event, true);
            return;
        }
        if route.subscribers.len() == 1 {
            route.subscribers[0].1.push(event);
            return;
        }
        let Some((last, rest)) = route.subscribers.split_last() else {
            unreachable!("checked non-empty subscriber route");
        };
        for (_, queue) in rest {
            queue.push_shared(event.clone());
        }
        last.1.push_shared(event);
    }

    pub(crate) fn close(&self) {
        let queues = self
            .channels
            .lock()
            .expect("thread event registry poisoned")
            .drain()
            .flat_map(|(_, route)| route.subscribers.into_iter().map(|(_, queue)| queue))
            .collect::<Vec<_>>();
        for queue in queues {
            queue.close();
        }
    }
}

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
