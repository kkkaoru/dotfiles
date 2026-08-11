use std::time::Duration;

use crate::app_server::ThreadEvents;
use serde_json::Value;

const EXTERNAL_TOOL_BATCH_HANDOFF_QUIET_PERIOD: Duration = Duration::from_millis(5);

pub(super) enum NextEvent {
    Event(Value),
    /// Ends the current response segment, not the provider turn. Any unread or later
    /// thread events are handed off through the dispatcher backlog to the next turn.
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
    match tokio::time::timeout(EXTERNAL_TOOL_BATCH_HANDOFF_QUIET_PERIOD, events.recv()).await {
        Ok(event) => classify_event(event),
        Err(_) => NextEvent::ExternalBatchReady,
    }
}

fn classify_event(event: Option<Value>) -> NextEvent {
    event.map_or(NextEvent::Closed, NextEvent::Event)
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::{EXTERNAL_TOOL_BATCH_HANDOFF_QUIET_PERIOD, NextEvent, classify_event, next_event};
    use crate::app_server::events::ThreadEventDispatcher;

    #[test]
    fn classifies_a_closed_event_source() {
        assert!(matches!(classify_event(None), NextEvent::Closed));
    }

    #[tokio::test]
    async fn hands_events_after_the_quiet_boundary_to_the_next_turn() {
        tokio::time::pause();
        let dispatcher = ThreadEventDispatcher::default();
        let events = Arc::new(dispatcher.subscribe("thread"));
        dispatcher.dispatch(tool_event("first"));
        assert_event_call_id(&events, "first").await;

        let boundary = wait_for_external_batch_ready(Arc::clone(&events)).await;
        assert!(matches!(boundary, NextEvent::ExternalBatchReady));

        dispatcher.dispatch(tool_event("queued-before-drop"));
        drop(events);
        dispatcher.dispatch(tool_event("arrived-after-drop"));

        let next_turn = dispatcher.subscribe("thread");
        assert_event_call_id(&next_turn, "queued-before-drop").await;
        assert_event_call_id(&next_turn, "arrived-after-drop").await;
    }

    async fn wait_for_external_batch_ready(
        events: Arc<crate::app_server::ThreadEvents>,
    ) -> NextEvent {
        let waiting_events = Arc::clone(&events);
        let boundary = tokio::spawn(async move { next_event(&waiting_events, true).await });
        tokio::task::yield_now().await;
        tokio::time::advance(EXTERNAL_TOOL_BATCH_HANDOFF_QUIET_PERIOD).await;
        boundary.await.expect("quiet-boundary task")
    }

    async fn assert_event_call_id(events: &crate::app_server::ThreadEvents, call_id: &str) {
        let NextEvent::Event(event) = next_event(events, false).await else {
            panic!("expected NextEvent::Event for {call_id}");
        };
        assert_eq!(event["params"]["callId"], call_id);
    }

    fn tool_event(call_id: &str) -> serde_json::Value {
        json!({
            "method":"item/tool/call",
            "params":{"threadId":"thread","callId":call_id,"tool":"Read"}
        })
    }
}
