use std::sync::Mutex;

use serde_json::Value;
use tokio::sync::Notify;

use super::QueueState;

#[derive(Default)]
pub(super) struct EventQueue {
    pub(super) state: Mutex<QueueState>,
    pub(super) ready: Notify,
}

pub(super) enum QueuePoll {
    Event(Value),
    Closed,
    Pending,
}

impl EventQueue {
    pub(super) fn push(&self, event: Value) {
        self.push_with_requeueability(event, true);
    }

    pub(super) fn push_shared(&self, event: Value) {
        self.push_with_requeueability(event, false);
    }

    pub(super) fn push_with_requeueability(&self, event: Value, requeueable: bool) {
        let mut state = self.state.lock().expect("thread event queue poisoned");
        if state.closed || state.overflowed || state.terminal_seen {
            return;
        }
        state.push_or_overflow(event, requeueable);
        drop(state);
        self.ready.notify_one();
    }

    pub(super) async fn recv(&self) -> Option<Value> {
        loop {
            let notified = self.ready.notified();
            match self.poll() {
                QueuePoll::Event(event) => return Some(event),
                QueuePoll::Closed => return None,
                QueuePoll::Pending => notified.await,
            }
        }
    }

    pub(super) fn poll(&self) -> QueuePoll {
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

    pub(super) fn close(&self) {
        self.state
            .lock()
            .expect("thread event queue poisoned")
            .closed = true;
        self.ready.notify_waiters();
    }
}
