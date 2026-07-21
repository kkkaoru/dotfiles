use std::{
    collections::{HashMap, VecDeque},
    io::{self, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde_json::{Value, json};
use tokio::sync::Notify;

const MAX_QUEUED_EVENTS: usize = 256;
const MAX_QUEUED_BYTES: usize = 1024 * 1024;

type Subscribers = Vec<(u64, Arc<EventQueue>)>;
type Registry = HashMap<String, Subscribers>;

#[derive(Default)]
struct QueueState {
    events: VecDeque<QueuedEvent>,
    queued_bytes: usize,
    closed: bool,
    overflowed: bool,
}

struct QueuedEvent {
    value: Value,
    bytes: usize,
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
        let mut state = self.state.lock().expect("thread event queue poisoned");
        if state.closed || state.overflowed {
            return;
        }
        state.push_or_overflow(event);
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
    fn push_or_overflow(&mut self, event: Value) {
        if let Some(suffix) = self
            .events
            .back()
            .and_then(|last| coalescible_suffix(&last.value, &event))
        {
            if !self.append_delta(suffix) {
                self.overflow(event_thread_id(&event).expect("dispatched event thread id"));
            }
            return;
        }

        let bytes = event_bytes(&event);
        if self.events.len() >= MAX_QUEUED_EVENTS
            || self.queued_bytes.saturating_add(bytes) > MAX_QUEUED_BYTES
        {
            self.overflow(event_thread_id(&event).expect("dispatched event thread id"));
            return;
        }
        self.events.push_back(QueuedEvent {
            value: event,
            bytes,
        });
        self.queued_bytes += bytes;
    }

    fn append_delta(&mut self, suffix: &str) -> bool {
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
        self.queued_bytes += additional_bytes;
        true
    }

    fn overflow(&mut self, thread_id: &str) {
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
        });
        self.queued_bytes = bytes;
        self.overflowed = true;
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
        let queue = Arc::new(EventQueue::default());
        self.channels
            .lock()
            .expect("thread event registry poisoned")
            .entry(thread_id.to_owned())
            .or_insert_with(|| Vec::with_capacity(1))
            .push((channel_id, Arc::clone(&queue)));
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
        let channels = self
            .channels
            .lock()
            .expect("thread event registry poisoned");
        let Some(entries) = channels.get(thread_id) else {
            return;
        };
        if entries.len() == 1 {
            let queue = Arc::clone(&entries[0].1);
            drop(channels);
            queue.push(event);
            return;
        }
        let mut subscribers = entries
            .iter()
            .map(|(_, queue)| Arc::clone(queue))
            .collect::<Vec<_>>();
        drop(channels);
        let Some(last) = subscribers.pop() else {
            return;
        };
        for queue in subscribers {
            queue.push(event.clone());
        }
        last.push(event);
    }

    pub(crate) fn close(&self) {
        let queues = self
            .channels
            .lock()
            .expect("thread event registry poisoned")
            .drain()
            .flat_map(|(_, subscribers)| subscribers.into_iter().map(|(_, queue)| queue))
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
    pub async fn recv(&self) -> Option<Value> {
        self.queue.recv().await
    }
}

impl Drop for ThreadEvents {
    fn drop(&mut self) {
        {
            let mut channels = self
                .channels
                .lock()
                .expect("thread event registry poisoned");
            let is_empty = channels
                .get_mut(&self.thread_id)
                .is_some_and(|subscribers| {
                    subscribers.retain(|(channel_id, _)| *channel_id != self.channel_id);
                    subscribers.is_empty()
                });
            if is_empty {
                channels.remove(&self.thread_id);
            }
        }
        self.queue.close();
    }
}

fn coalescible_suffix<'a>(last: &Value, next: &'a Value) -> Option<&'a str> {
    let method = last.get("method")?.as_str()?;
    if next.get("method")?.as_str()? != method
        || !matches!(
            method,
            "item/agentMessage/delta" | "item/reasoning/summaryTextDelta"
        )
        || last.pointer("/params/turnId") != next.pointer("/params/turnId")
        || last.pointer("/params/itemId") != next.pointer("/params/itemId")
        || (method == "item/reasoning/summaryTextDelta"
            && last.pointer("/params/summaryIndex") != next.pointer("/params/summaryIndex"))
    {
        return None;
    }
    last.pointer("/params/delta")?.as_str()?;
    next.pointer("/params/delta")?.as_str()
}

fn is_bridge_event(event: &Value) -> bool {
    // App-server lifecycle events can repeat the complete user input and dynamic tool schemas.
    // The Anthropic bridge ignores them, so admitting them can overflow the queue before the
    // small output deltas behind them are consumed.
    matches!(
        event.get("method").and_then(Value::as_str),
        Some(
            "item/agentMessage/delta"
                | "item/reasoning/summaryTextDelta"
                | "item/tool/call"
                | "item/providerTool/call"
                | "item/providerTool/update"
                | "thread/tokenUsage/updated"
                | "turn/completed"
                | "error"
        )
    )
}

fn encoded_string_content_bytes(value: &str) -> usize {
    encoded_bytes(value).saturating_sub(2)
}

fn event_bytes(event: &Value) -> usize {
    encoded_bytes(event)
}

fn encoded_bytes(value: &(impl serde::Serialize + ?Sized)) -> usize {
    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, value).map_or(usize::MAX, |()| counter.bytes)
}

#[derive(Default)]
struct ByteCounter {
    bytes: usize,
}

impl Write for ByteCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buffer.len());
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn event_thread_id(event: &Value) -> Option<&str> {
    event
        .pointer("/params/threadId")
        .and_then(Value::as_str)
        .or_else(|| {
            event
                .pointer("/params/turn/threadId")
                .and_then(Value::as_str)
        })
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
