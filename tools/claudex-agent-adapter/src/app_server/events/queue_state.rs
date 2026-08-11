use serde_json::{Value, json};

use super::{
    MAX_COALESCED_DELTA_CHARS, MAX_QUEUED_BYTES, MAX_QUEUED_EVENTS, QueueState, QueuedEvent,
};
use super::encoding::{encoded_string_content_bytes, event_bytes};
use super::event_shape::{coalescible_suffix, event_thread_id, is_terminal_event};

impl QueueState {
    pub(super) fn push_or_overflow(&mut self, event: Value, requeueable: bool) {
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

    pub(super) fn append_delta(&mut self, suffix: &str, requeueable: bool) -> bool {
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

    pub(super) fn take_requeueable_backlog(&mut self) -> Self {
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
