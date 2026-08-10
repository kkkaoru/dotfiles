use std::{
    collections::{HashMap, VecDeque},
    mem,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde_json::{Value, json};
use tokio::sync::Notify;

use self::encoding::{encoded_string_content_bytes, event_bytes};
use self::event_shape::{coalescible_suffix, event_thread_id, is_bridge_event, is_terminal_event};

mod encoding;
mod event_shape;

const MAX_QUEUED_EVENTS: usize = 256;
const MAX_QUEUED_BYTES: usize = 1024 * 1024;
/// Cap coalesced text-delta size so Claude Code paints GPT/Codex progress in
/// frequent chunks instead of one stalled mega-frame when the consumer lags.
const MAX_COALESCED_DELTA_CHARS: usize = 256;

type Subscribers = Vec<(u64, Arc<EventQueue>)>;
type Registry = HashMap<String, ThreadRoute>;

#[derive(Default)]
struct ThreadRoute {
    subscribers: Subscribers,
    backlog: QueueState,
}

#[derive(Default)]
struct QueueState {
    events: VecDeque<QueuedEvent>,
    queued_bytes: usize,
    closed: bool,
    overflowed: bool,
    terminal_seen: bool,
}

struct QueuedEvent {
    value: Value,
    bytes: usize,
    requeueable: bool,
}

#[derive(Default)]
struct EventQueue {
    state: Mutex<QueueState>,
    ready: Notify,
}

enum QueuePoll {
    Event(Value),
    Closed,
    Pending,
}

impl EventQueue {
    fn push(&self, event: Value) {
        self.push_with_requeueability(event, true);
    }

    fn push_shared(&self, event: Value) {
        self.push_with_requeueability(event, false);
    }

    fn push_with_requeueability(&self, event: Value, requeueable: bool) {
        let mut state = self.state.lock().expect("thread event queue poisoned");
        if state.closed || state.overflowed || state.terminal_seen {
            return;
        }
        state.push_or_overflow(event, requeueable);
        drop(state);
        self.ready.notify_one();
    }

    async fn recv(&self) -> Option<Value> {
        loop {
            let notified = self.ready.notified();
            match self.poll() {
                QueuePoll::Event(event) => return Some(event),
                QueuePoll::Closed => return None,
                QueuePoll::Pending => notified.await,
            }
        }
    }

    fn poll(&self) -> QueuePoll {
        let mut state = self.state.lock().expect("thread event queue poisoned");
        if let Some(event) = state.events.pop_front() {
            state.queued_bytes -= event.bytes;
            return QueuePoll::Event(event.value);
        }
        if state.closed {
            QueuePoll::Closed
        } else {
            QueuePoll::Pending
        }
    }

    fn close(&self) {
        self.state
            .lock()
            .expect("thread event queue poisoned")
            .closed = true;
        self.ready.notify_waiters();
    }
}

impl QueueState {
    fn push_or_overflow(&mut self, event: Value, requeueable: bool) {
        if self.overflowed || self.terminal_seen {
            return;
        }
        if self.append_to_coalescible_tail(&event, requeueable) {
            return;
        }

        let bytes = event_bytes(&event);
        let terminal = is_terminal_event(&event);
        if self.events.len() >= MAX_QUEUED_EVENTS
            || self.queued_bytes.saturating_add(bytes) > MAX_QUEUED_BYTES
        {
            self.overflow(
                event_thread_id(&event).expect("dispatched event thread id"),
                requeueable,
            );
            self.terminal_seen = terminal;
            return;
        }
        self.events.push_back(QueuedEvent {
            value: event,
            bytes,
            requeueable,
        });
        self.queued_bytes += bytes;
        self.terminal_seen = terminal;
    }

    fn append_to_coalescible_tail(&mut self, event: &Value, requeueable: bool) -> bool {
        let Some(suffix) = self
            .events
            .back()
            .and_then(|last| coalescible_suffix(&last.value, event))
        else {
            return false;
        };
        let current_len = self
            .events
            .back()
            .and_then(|last| last.value.pointer("/params/delta"))
            .and_then(Value::as_str)
            .map_or(0, str::len);
        if current_len > 0
            && current_len.saturating_add(suffix.len()) > MAX_COALESCED_DELTA_CHARS
        {
            return false;
        }
        if self.append_delta(suffix, requeueable) {
            return true;
        }
        self.overflow(
            event_thread_id(event).expect("dispatched event thread id"),
            requeueable,
        );
        true
    }

    fn append_delta(&mut self, suffix: &str, requeueable: bool) -> bool {
        let additional_bytes = encoded_string_content_bytes(suffix);
        if self.queued_bytes.saturating_add(additional_bytes) > MAX_QUEUED_BYTES {
            return false;
        }
        let event = self.events.back_mut().expect("coalescible queue tail");
        let Value::String(delta) = event
            .value
            .pointer_mut("/params/delta")
            .expect("coalescible text delta")
        else {
            unreachable!("coalescible delta is a string");
        };
        delta.push_str(suffix);
        event.bytes += additional_bytes;
        event.requeueable &= requeueable;
        self.queued_bytes += additional_bytes;
        true
    }

    fn overflow(&mut self, thread_id: &str, requeueable: bool) {
        let requeueable = requeueable && self.events.iter().all(|event| event.requeueable);
        let event = json!({
            "method":"error",
            "params":{
                "threadId":thread_id,
                "willRetry":false,
                "error":{"message":"claudex app-server event queue overflowed"}
            }
        });
        let bytes = event_bytes(&event);
        self.events.clear();
        self.events.push_back(QueuedEvent {
            value: event,
            bytes,
            requeueable,
        });
        self.queued_bytes = bytes;
        self.overflowed = true;
    }

    fn take_requeueable_backlog(&mut self) -> Self {
        if self.terminal_seen {
            self.events.clear();
            self.queued_bytes = 0;
            return Self::default();
        }
        let mut backlog = Self::default();
        backlog.events = self
            .events
            .drain(..)
            .filter(|event| event.requeueable)
            .collect();
        backlog.queued_bytes = backlog.events.iter().map(|event| event.bytes).sum();
        self.queued_bytes = 0;
        backlog.overflowed = self.overflowed && !backlog.events.is_empty();
        backlog
    }
}

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

fn unsubscribe(channels: &mut Registry, thread_id: &str, channel_id: u64, queue: &EventQueue) {
    let remove_route = match channels.get_mut(thread_id) {
        Some(route) => requeue_if_last_subscriber(route, channel_id, queue),
        None => false,
    };
    if remove_route {
        channels.remove(thread_id);
    }
}

fn requeue_if_last_subscriber(
    route: &mut ThreadRoute,
    channel_id: u64,
    queue: &EventQueue,
) -> bool {
    route
        .subscribers
        .retain(|(registered_id, _)| *registered_id != channel_id);
    if !route.subscribers.is_empty() {
        return false;
    }
    let mut state = queue.state.lock().expect("thread event queue poisoned");
    debug_assert!(route.backlog.events.is_empty());
    route.backlog = state.take_requeueable_backlog();
    route.backlog.events.is_empty()
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
