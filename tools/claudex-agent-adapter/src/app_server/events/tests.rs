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

fn reasoning_text_delta(thread_id: &str, index: u64, text: &str) -> Value {
    json!({
        "method":"item/reasoning/textDelta",
        "params":{
            "threadId":thread_id,"turnId":"turn","itemId":"reasoning",
            "contentIndex":index,"delta":text
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

#[test]
fn ignores_events_after_overflow_or_terminal_state() {
    let event = json!({"method":"item/tool/call"});
    let mut overflowed = QueueState {
        overflowed: true,
        ..QueueState::default()
    };
    overflowed.push_or_overflow(event.clone(), true);
    assert!(overflowed.events.is_empty());

    let mut terminal = QueueState {
        terminal_seen: true,
        ..QueueState::default()
    };
    terminal.push_or_overflow(event, true);
    assert!(terminal.events.is_empty());
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
async fn replays_a_bounded_pre_subscription_backlog_in_fifo_order() {
    let dispatcher = ThreadEventDispatcher::default();
    for sequence in 0..3 {
        dispatcher.dispatch(json!({
            "method":"item/tool/call",
            "params":{"threadId":"backlog","sequence":sequence}
        }));
    }

    let events = dispatcher.subscribe("backlog");
    for sequence in 0..3 {
        assert_eq!(events.recv().await.unwrap()["params"]["sequence"], sequence);
    }

    drop(events);
    for sequence in 0..=MAX_QUEUED_EVENTS {
        dispatcher.dispatch(json!({
            "method":"item/tool/call",
            "params":{"threadId":"backlog","sequence":sequence}
        }));
    }
    let events = dispatcher.subscribe("backlog");
    let overflow = events.recv().await.unwrap();
    assert_eq!(overflow["method"], "error");
    assert!(events.queue.state.lock().unwrap().overflowed);
}

#[tokio::test]
async fn hands_unread_single_subscriber_events_to_the_next_turn() {
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("handoff");
    dispatcher.dispatch(json!({
        "method":"item/tool/call",
        "params":{"threadId":"handoff","callId":"late"}
    }));
    drop(events);

    let next_turn = dispatcher.subscribe("handoff");
    assert_eq!(next_turn.recv().await.unwrap()["params"]["callId"], "late");
}

#[tokio::test]
async fn does_not_replay_fanout_copies_or_events_from_a_terminal_turn() {
    let dispatcher = ThreadEventDispatcher::default();
    let first = dispatcher.subscribe("shared-handoff");
    let second = dispatcher.subscribe("shared-handoff");
    dispatcher.dispatch(json!({
        "method":"item/tool/call",
        "params":{"threadId":"shared-handoff","callId":"shared"}
    }));
    assert_eq!(first.recv().await.unwrap()["params"]["callId"], "shared");
    drop(first);
    drop(second);
    let next_turn = dispatcher.subscribe("shared-handoff");
    assert!(next_turn.queue.state.lock().unwrap().events.is_empty());

    let terminal = dispatcher.subscribe("terminal");
    dispatcher.dispatch(json!({
        "method":"item/tool/call",
        "params":{"threadId":"terminal","callId":"obsolete"}
    }));
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"turn":{"threadId":"terminal","status":"completed"}}
    }));
    drop(terminal);
    let after_terminal = dispatcher.subscribe("terminal");
    assert!(after_terminal.queue.state.lock().unwrap().events.is_empty());
}

#[tokio::test]
async fn coalesces_a_stalled_burst_into_progress_sized_chunks() {
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("burst");
    for _ in 0..4096 {
        dispatcher.dispatch(delta("burst", "x"));
    }

    let mut total = 0usize;
    let mut chunks = 0usize;
    while !events.queue.state.lock().unwrap().events.is_empty() {
        let event = events.recv().await.unwrap();
        let len = event["params"]["delta"].as_str().unwrap().len();
        assert!(len <= MAX_COALESCED_DELTA_CHARS);
        total += len;
        chunks += 1;
    }
    assert_eq!(total, 4096);
    assert!(chunks >= 4096 / MAX_COALESCED_DELTA_CHARS);
}

#[tokio::test]
async fn coalesces_reasoning_bursts_but_preserves_summary_boundaries() {
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("reasoning");
    for _ in 0..4096 {
        dispatcher.dispatch(reasoning_delta("reasoning", 0, "x"));
    }
    dispatcher.dispatch(reasoning_delta("reasoning", 1, "next"));

    let summary0 = sum_coalesced_deltas_until_summary(&events, 1, "next").await;
    assert_eq!(summary0, 4096);
}

async fn sum_coalesced_deltas_until_summary(
    events: &ThreadEvents,
    summary_index: i64,
    expected_delta: &str,
) -> usize {
    let mut total = 0usize;
    loop {
        let event = events.recv().await.unwrap();
        if event["params"]["summaryIndex"] == summary_index {
            assert_eq!(event["params"]["delta"], expected_delta);
            return total;
        }
        let len = event["params"]["delta"].as_str().unwrap().len();
        assert!(len <= MAX_COALESCED_DELTA_CHARS);
        total += len;
    }
}

#[tokio::test]
async fn coalesces_reasoning_text_deltas_but_preserves_content_boundaries() {
    let dispatcher = ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("reasoning-text");
    for _ in 0..64 {
        dispatcher.dispatch(reasoning_text_delta("reasoning-text", 0, "a"));
    }
    dispatcher.dispatch(reasoning_text_delta("reasoning-text", 1, "b"));

    let first = events.recv().await.unwrap();
    let second = events.recv().await.unwrap();
    assert_eq!(first["params"]["delta"].as_str().unwrap().len(), 64);
    assert_eq!(first["params"]["contentIndex"], 0);
    assert_eq!(second["params"]["delta"], "b");
    assert_eq!(second["params"]["contentIndex"], 1);
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
    dispatcher.dispatch(json!({"method":"error","params":{}}));
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
        "item/reasoning/textDelta",
        "item/reasoning/complete",
        "item/tool/call",
        "item/tool/start",
        "item/tool/delta",
        "thread/tokenUsage/updated",
        "turn/completed",
        "error",
    ] {
        assert!(is_bridge_event(&json!({ "method": method })));
    }
    for method in ["thread/started", "turn/started"] {
        assert!(!is_bridge_event(&json!({ "method": method })));
    }
    for method in ["item/started", "item/completed"] {
        assert!(is_bridge_event(&json!({
            "method": method,
            "params": {"item": {"type": "webSearch"}}
        })));
        assert!(!is_bridge_event(&json!({
            "method": method,
            "params": {"item": {"type": "agentMessage"}}
        })));
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
            .subscribers
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
            requeueable: true,
        }]),
        ..QueueState::default()
    };
    state.append_delta("suffix", true);
}

#[tokio::test]
async fn closed_terminal_and_empty_routes_do_not_requeue_events() {
    let queue = Arc::new(EventQueue {
        state: Mutex::new(QueueState::default()),
        ready: Notify::new(),
    });
    queue.close();
    queue.push(delta("closed", "ignored"));
    queue.push_shared(delta("closed", "ignored"));
    assert!(queue.recv().await.is_none());

    let mut terminal = QueueState {
        terminal_seen: true,
        ..QueueState::default()
    };
    terminal.push_or_overflow(delta("terminal", "ignored"), true);
    assert!(terminal.events.is_empty());
    assert!(terminal.take_requeueable_backlog().events.is_empty());

    let mut overflow = QueueState {
        overflowed: true,
        events: VecDeque::from([QueuedEvent {
            value: delta("overflow", "kept"),
            bytes: event_bytes(&delta("overflow", "kept")),
            requeueable: true,
        }]),
        queued_bytes: event_bytes(&delta("overflow", "kept")),
        ..QueueState::default()
    };
    let backlog = overflow.take_requeueable_backlog();
    assert!(backlog.overflowed);
    assert_eq!(backlog.events.len(), 1);
}

#[tokio::test]
async fn queue_poll_covers_pending_event_and_closed_states() {
    let queue = EventQueue::default();
    assert!(matches!(queue.poll(), QueuePoll::Pending));
    queue.push(delta("poll", "value"));
    assert!(matches!(queue.poll(), QueuePoll::Event(value) if value["params"]["delta"] == "value"));
    queue.close();
    assert!(matches!(queue.poll(), QueuePoll::Closed));

    for state in [
        QueueState {
            overflowed: true,
            ..QueueState::default()
        },
        QueueState {
            terminal_seen: true,
            ..QueueState::default()
        },
    ] {
        let queue = EventQueue {
            state: Mutex::new(state),
            ready: Notify::new(),
        };
        queue.push(delta("blocked", "ignored"));
        assert!(matches!(queue.poll(), QueuePoll::Pending));
    }
}

#[test]
fn coalesces_an_empty_delta_then_a_suffix_without_hitting_the_char_cap() {
    let mut state = QueueState::default();
    state.push_or_overflow(delta("empty-delta", ""), true);
    state.push_or_overflow(delta("empty-delta", "x"), true);
    assert_eq!(state.events.len(), 1);
    assert_eq!(state.events[0].value["params"]["delta"], "x");
}

#[test]
fn append_delta_byte_cap_overflows_mixed_requeueable_events() {
    let mut state = QueueState::default();
    state.push_or_overflow(delta("mix-bytes", "hi"), false);
    assert_eq!(state.events.len(), 1);
    state.queued_bytes = MAX_QUEUED_BYTES;
    state.push_or_overflow(delta("mix-bytes", "x"), true);
    assert!(state.overflowed);
    assert_eq!(state.events.len(), 1);
    assert_eq!(state.events[0].value["method"], "error");
    assert!(
        !state.events[0].requeueable,
        "a non-requeueable tail must keep overflow non-requeueable"
    );
}
