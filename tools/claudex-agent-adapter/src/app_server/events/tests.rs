use serde_json::json;

use super::*;

fn delta(thread_id: &str, text: &str) -> Value {
    json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":thread_id,"turnId":"turn","itemId":"item","delta":text}
    })
}

fn reasoning_delta(thread_id: &str, index: u64, text: &str) -> Value {
    json!({
        "method":"item/reasoning/summaryTextDelta",
        "params":{
            "threadId":thread_id,"turnId":"turn","itemId":"reasoning",
            "summaryIndex":index,"delta":text
        }
    })
}

#[test]
fn counts_encoded_bytes_without_materializing_json() {
    for value in [
        json!(null),
        json!({"plain":"text"}),
        json!({"escaped":"quote\" newline\n"}),
        json!({"unicode":"日本語"}),
    ] {
        assert_eq!(
            event_bytes(&value),
            serde_json::to_vec(&value).unwrap().len()
        );
    }
    for value in ["plain", "quote\" newline\n", "日本語"] {
        assert_eq!(
            encoded_string_content_bytes(value),
            serde_json::to_vec(value).unwrap().len() - 2
        );
    }
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
async fn coalesces_reasoning_bursts_but_preserves_summary_boundaries() {
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("reasoning");
    for _ in 0..4096 {
        dispatcher.dispatch(reasoning_delta("reasoning", 0, "x"));
    }
    dispatcher.dispatch(reasoning_delta("reasoning", 1, "next"));

    let first = events.recv().await.unwrap();
    let second = events.recv().await.unwrap();
    assert_eq!(first["params"]["delta"].as_str().unwrap().len(), 4096);
    assert_eq!(second["params"]["delta"], "next");
}

#[tokio::test]
async fn reports_non_coalescible_overflow_explicitly() {
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("overflow");
    for sequence in 0..=MAX_QUEUED_EVENTS {
        dispatcher.dispatch(json!({
            "method":"item/tool/call",
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
        "method":"item/tool/call",
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
            "method":"item/tool/call",
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
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"turn":{"threadId":"nested","status":"completed"}}
    }));
    assert!(events.recv().await.is_some());
    drop(events);
    assert!(dispatcher.channels.lock().unwrap().is_empty());

    let events = dispatcher.subscribe("closed");
    dispatcher.dispatch(json!({"params":{}}));
    dispatcher.close();
    assert!(events.recv().await.is_none());
}

#[tokio::test]
async fn discards_oversized_events_the_bridge_never_consumes() {
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("filtered");
    dispatcher.dispatch(json!({
        "method":"item/started",
        "params":{
            "threadId":"filtered",
            "item":{"input":"x".repeat(MAX_QUEUED_BYTES * 2)}
        }
    }));
    dispatcher.dispatch(delta("filtered", "answer"));

    assert_eq!(events.recv().await.unwrap()["params"]["delta"], "answer");
    let state = events.queue.state.lock().unwrap();
    assert!(!state.overflowed);
    assert!(state.events.is_empty());
}

#[test]
fn accepts_only_events_used_by_the_anthropic_bridge() {
    for method in [
        "item/agentMessage/delta",
        "item/reasoning/summaryTextDelta",
        "item/tool/call",
        "thread/tokenUsage/updated",
        "turn/completed",
        "error",
    ] {
        assert!(is_bridge_event(&json!({"method":method})));
    }
    for method in [
        "thread/started",
        "turn/started",
        "item/started",
        "item/completed",
        "item/reasoning/textDelta",
    ] {
        assert!(!is_bridge_event(&json!({"method":method})));
    }
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
    second.queue.push(delta("shared", "ignored"));
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
    state.append_delta("suffix");
}
