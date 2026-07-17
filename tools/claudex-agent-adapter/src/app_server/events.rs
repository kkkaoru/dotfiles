use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde_json::{Value, json};
use tokio::sync::Notify;

const MAX_QUEUED_EVENTS: usize = 256;
const MAX_QUEUED_BYTES: usize = 1024 * 1024;

type Subscribers = HashMap<u64, Arc<EventQueue>>;
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
    fn push(&self, event: Value, thread_id: &str) {
        let mut state = self.state.lock().expect("thread event queue poisoned");
        if state.closed || state.overflowed {
            return;
        }
        state.push_or_overflow(event, thread_id);
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
    fn push_or_overflow(&mut self, event: Value, thread_id: &str) {
        if let Some(suffix) = self
            .events
            .back()
            .and_then(|last| coalescible_suffix(&last.value, &event))
        {
            self.append_delta_or_overflow(suffix, thread_id);
            return;
        }

        let bytes = event_bytes(&event);
        if self.events.len() >= MAX_QUEUED_EVENTS
            || self.queued_bytes.saturating_add(bytes) > MAX_QUEUED_BYTES
        {
            self.overflow(thread_id);
            return;
        }
        self.events.push_back(QueuedEvent {
            value: event,
            bytes,
        });
        self.queued_bytes += bytes;
    }

    fn append_delta_or_overflow(&mut self, suffix: &str, thread_id: &str) {
        let additional_bytes = encoded_string_content_bytes(suffix);
        if self.queued_bytes.saturating_add(additional_bytes) > MAX_QUEUED_BYTES {
            self.overflow(thread_id);
            return;
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
            .or_default()
            .insert(channel_id, Arc::clone(&queue));
        ThreadEvents {
            thread_id: thread_id.to_owned(),
            channel_id,
            queue,
            channels: Arc::clone(&self.channels),
        }
    }

    pub(crate) fn dispatch(&self, event: Value) {
        let Some(thread_id) = event_thread_id(&event).map(str::to_owned) else {
            tracing::debug!(?event, "ignored app-server event without thread id");
            return;
        };
        let mut subscribers = self
            .channels
            .lock()
            .expect("thread event registry poisoned")
            .get(&thread_id)
            .map(|entries| entries.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let Some(last) = subscribers.pop() else {
            return;
        };
        for queue in subscribers {
            queue.push(event.clone(), &thread_id);
        }
        last.push(event, &thread_id);
    }

    pub(crate) fn close(&self) {
        let queues = self
            .channels
            .lock()
            .expect("thread event registry poisoned")
            .drain()
            .flat_map(|(_, subscribers)| subscribers.into_values())
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
                    subscribers.remove(&self.channel_id);
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
    if last.get("method")?.as_str()? != "item/agentMessage/delta"
        || next.get("method")?.as_str()? != "item/agentMessage/delta"
        || last.pointer("/params/turnId") != next.pointer("/params/turnId")
        || last.pointer("/params/itemId") != next.pointer("/params/itemId")
    {
        return None;
    }
    last.pointer("/params/delta")?.as_str()?;
    next.pointer("/params/delta")?.as_str()
}

fn encoded_string_content_bytes(value: &str) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |encoded| encoded.len().saturating_sub(2))
}

fn event_bytes(event: &Value) -> usize {
    serde_json::to_vec(event).map_or(usize::MAX, |bytes| bytes.len())
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
mod tests {
    use serde_json::json;

    use super::*;

    fn delta(thread_id: &str, text: &str) -> Value {
        json!({
            "method":"item/agentMessage/delta",
            "params":{"threadId":thread_id,"turnId":"turn","itemId":"item","delta":text}
        })
    }

    #[tokio::test]
    async fn isolates_threads_and_fans_out_subscribers() {
        let dispatcher = ThreadEventDispatcher::default();
        let first = dispatcher.subscribe("shared");
        let second = dispatcher.subscribe("shared");
        let other = dispatcher.subscribe("other");
        dispatcher.dispatch(delta("shared", "text"));

        assert_eq!(first.recv().await.unwrap()["params"]["delta"], "text");
        assert_eq!(second.recv().await.unwrap()["params"]["delta"], "text");
        assert!(other.queue.state.lock().unwrap().events.is_empty());
    }

    #[tokio::test]
    async fn coalesces_a_stalled_burst_larger_than_the_queue_limit() {
        let dispatcher = ThreadEventDispatcher::default();
        let events = dispatcher.subscribe("burst");
        for _ in 0..4096 {
            dispatcher.dispatch(delta("burst", "x"));
        }

        let event = events.recv().await.unwrap();
        assert_eq!(event["params"]["delta"].as_str().unwrap().len(), 4096);
        assert!(events.queue.state.lock().unwrap().events.is_empty());
    }

    #[tokio::test]
    async fn reports_non_coalescible_overflow_explicitly() {
        let dispatcher = ThreadEventDispatcher::default();
        let events = dispatcher.subscribe("overflow");
        for sequence in 0..=MAX_QUEUED_EVENTS {
            dispatcher.dispatch(json!({
                "method":"fixture/event",
                "params":{"threadId":"overflow","sequence":sequence}
            }));
        }

        let event = events.recv().await.unwrap();
        assert_eq!(event["method"], "error");
        assert_eq!(event["params"]["willRetry"], false);
        assert!(
            event["params"]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("overflowed")
        );
        let state = events.queue.state.lock().unwrap();
        assert!(state.overflowed);
        assert!(state.queued_bytes <= MAX_QUEUED_BYTES);
    }

    #[tokio::test]
    async fn caps_a_single_oversized_delta() {
        let dispatcher = ThreadEventDispatcher::default();
        let events = dispatcher.subscribe("bytes");
        dispatcher.dispatch(delta("bytes", &"x".repeat(MAX_QUEUED_BYTES)));

        let event = events.recv().await.unwrap();
        assert_eq!(event["method"], "error");
        let state = events.queue.state.lock().unwrap();
        assert!(state.overflowed);
        assert!(state.queued_bytes <= MAX_QUEUED_BYTES);
    }

    #[tokio::test]
    async fn caps_coalesced_deltas_and_ignores_later_events() {
        let dispatcher = ThreadEventDispatcher::default();
        let events = dispatcher.subscribe("coalesced-bytes");
        dispatcher.dispatch(delta("coalesced-bytes", "first"));
        dispatcher.dispatch(delta("coalesced-bytes", &"x".repeat(MAX_QUEUED_BYTES)));
        dispatcher.dispatch(json!({
            "method":"fixture/ignored-after-overflow",
            "params":{"threadId":"coalesced-bytes"}
        }));

        let event = events.recv().await.unwrap();
        assert_eq!(event["method"], "error");
        assert!(events.queue.state.lock().unwrap().events.is_empty());
    }

    #[tokio::test]
    async fn keeps_nonmatching_or_non_string_deltas_separate() {
        let dispatcher = ThreadEventDispatcher::default();
        let events = dispatcher.subscribe("separate");
        dispatcher.dispatch(delta("separate", "base"));
        let cases = [
            json!({
                "method":"item/agentMessage/delta",
                "params":{"threadId":"separate","turnId":"other","itemId":"item","delta":"b"}
            }),
            json!({
                "method":"item/agentMessage/delta",
                "params":{"threadId":"separate","turnId":"other","itemId":"other","delta":"c"}
            }),
            json!({
                "method":"item/agentMessage/delta",
                "params":{"threadId":"separate","turnId":"other","itemId":"other","delta":1}
            }),
            json!({
                "method":"item/agentMessage/delta",
                "params":{"threadId":"separate","turnId":"other","itemId":"other","delta":"d"}
            }),
            json!({
                "method":"fixture/event",
                "params":{"threadId":"separate","turnId":"other","itemId":"other","delta":"e"}
            }),
        ];
        for event in cases {
            dispatcher.dispatch(event);
        }

        for expected in ["base", "b", "c"] {
            assert_eq!(
                events.recv().await.unwrap()["params"]["delta"],
                json!(expected)
            );
        }
        assert_eq!(events.recv().await.unwrap()["params"]["delta"], json!(1));
        for expected in ["d", "e"] {
            assert_eq!(
                events.recv().await.unwrap()["params"]["delta"],
                json!(expected)
            );
        }
    }

    #[tokio::test]
    async fn supports_nested_ids_and_closes_or_cleans_channels() {
        let dispatcher = ThreadEventDispatcher::default();
        let events = dispatcher.subscribe("nested");
        dispatcher.dispatch(json!({"params":{"turn":{"threadId":"nested"}}}));
        assert!(events.recv().await.is_some());
        drop(events);
        assert!(dispatcher.channels.lock().unwrap().is_empty());

        let events = dispatcher.subscribe("closed");
        dispatcher.dispatch(json!({"params":{}}));
        dispatcher.close();
        assert!(events.recv().await.is_none());
    }

    #[tokio::test]
    async fn dropping_one_subscriber_retains_the_other_and_closes_its_queue() {
        let dispatcher = ThreadEventDispatcher::default();
        let first = dispatcher.subscribe("shared");
        let first_queue = Arc::clone(&first.queue);
        let second = dispatcher.subscribe("shared");
        drop(first);

        assert!(first_queue.state.lock().unwrap().closed);
        assert_eq!(
            dispatcher
                .channels
                .lock()
                .unwrap()
                .get("shared")
                .unwrap()
                .len(),
            1
        );
        dispatcher.dispatch(delta("shared", "remaining"));
        assert_eq!(second.recv().await.unwrap()["params"]["delta"], "remaining");

        dispatcher.close();
        dispatcher.close();
        assert!(second.recv().await.is_none());
        second.queue.push(delta("shared", "ignored"), "shared");
        assert!(second.queue.state.lock().unwrap().events.is_empty());
    }

    #[test]
    #[should_panic(expected = "coalescible delta is a string")]
    fn rejects_a_corrupted_coalescible_queue_tail() {
        let value = json!({"params":{"delta":1}});
        let mut state = QueueState {
            queued_bytes: event_bytes(&value),
            events: VecDeque::from([QueuedEvent {
                bytes: event_bytes(&value),
                value,
            }]),
            ..QueueState::default()
        };
        state.append_delta_or_overflow("suffix", "thread");
    }
}
