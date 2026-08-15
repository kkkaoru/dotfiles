use std::{
    mem,
    sync::{Arc, Mutex, atomic::Ordering},
};

use serde_json::Value;
use tokio::sync::Notify;

use super::{
    EventQueue, ThreadEventDispatcher, ThreadEvents, event_thread_id, is_bridge_event,
    is_terminal_event,
};

impl ThreadEventDispatcher {
    pub(crate) fn subscribe(&self, thread_id: &str) -> ThreadEvents {
        self.subscribe_with_drop(thread_id, None)
    }

    pub(crate) fn subscribe_with_drop(
        &self,
        thread_id: &str,
        on_drop: Option<Box<dyn FnOnce() + Send + Sync>>,
    ) -> ThreadEvents {
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
            on_drop,
        }
    }

    pub(crate) fn dispatch(&self, event: Value) {
        if !is_bridge_event(&event) {
            return;
        }
        let Some(thread_id) = event_thread_id(&event).map(str::to_owned) else {
            tracing::debug!(?event, "ignored app-server event without thread id");
            return;
        };
        self.dispatch_to(&thread_id, event);
    }

    pub(crate) fn dispatch_to(&self, route_id: &str, event: Value) {
        if !is_bridge_event(&event) {
            return;
        }
        let thread_id = route_id;
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
