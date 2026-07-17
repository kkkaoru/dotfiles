use std::time::Duration;

use crate::app_server::ThreadEvents;
use serde_json::Value;

const EXTERNAL_TOOL_BATCH_QUIET_PERIOD: Duration = Duration::from_millis(100);

pub(super) enum NextEvent {
    Event(Value),
    ExternalBatchReady,
    Closed,
}

/// App-server has no notification that marks the end of a group of dynamic tool calls.
/// `rawResponseItem/completed` describes one response item, so it cannot be that boundary.
pub(super) async fn next_event(
    events: &ThreadEvents,
    collecting_external_tools: bool,
) -> NextEvent {
    if !collecting_external_tools {
        return events
            .recv()
            .await
            .map_or(NextEvent::Closed, NextEvent::Event);
    }
    match tokio::time::timeout(EXTERNAL_TOOL_BATCH_QUIET_PERIOD, events.recv()).await {
        Ok(event) => classify_event(event),
        Err(_) => NextEvent::ExternalBatchReady,
    }
}

fn classify_event(event: Option<Value>) -> NextEvent {
    event.map_or(NextEvent::Closed, NextEvent::Event)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{NextEvent, classify_event};

    #[test]
    fn classifies_a_closed_event_source() {
        assert!(matches!(classify_event(None), NextEvent::Closed));
    }
}
