use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore};

use crate::agent_backend::{AcpLaunch, AgentBackend, BackendKind, BackendRoute, WebSearchMode};
use crate::parallel_scheduler::{ParallelScheduler, SchedulerConfig};

use super::*;

fn request(content: Value) -> MessagesRequest {
    MessagesRequest {
        model: "main-model".to_owned(),
        system: Value::Null,
        messages: vec![json!({"role":"user", "content":content})],
        tools: Vec::new(),
        stream: false,
        output_config: json!({}),
        metadata: json!({}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

fn launch_result(id: &str) -> Value {
    json!({
        "type":"tool_result",
        "tool_use_id":id,
        "content":[{"type":"text", "text":format!(
            "{ASYNC_LAUNCH_PREFIX}\nagentId: internal\n{BACKGROUND_MARKER}"
        )}]
    })
}

fn pure_async_launch_tool_results(message: &Value) -> Option<Vec<String>> {
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let blocks = message.get("content")?.as_array()?;
    if blocks.is_empty() {
        return None;
    }
    blocks
        .iter()
        .map(|block| {
            if block.get("type").and_then(Value::as_str) != Some("tool_result")
                || block.get("is_error").and_then(Value::as_bool) == Some(true)
            {
                return None;
            }
            let text = strict_result_text(block.get("content")?)?;
            if !text.trim_start().starts_with(ASYNC_LAUNCH_PREFIX)
                || !text.contains(BACKGROUND_MARKER)
            {
                return None;
            }
            block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        })
        .collect()
}

fn tool_round_ids(message: &Value) -> Option<Vec<String>> {
    let ids = message
        .get("content")?
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .map(|block| {
            block
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()?;
    (!ids.is_empty()).then_some(ids)
}

fn latest_tool_round_ids(request: &MessagesRequest) -> Option<Vec<String>> {
    request
        .messages
        .iter()
        .rev()
        .skip(1)
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .find_map(tool_round_ids)
}

#[test]
fn rejects_malformed_async_acknowledgement_shapes() {
    for message in [
        json!({"role":"assistant", "content":[]}),
        json!({"role":"user"}),
        json!({"role":"user", "content":[]} ),
        json!({"role":"user", "content":"not a block list"}),
        json!({"role":"user", "content":[{"type":"text","text":"not a result"}]}),
        json!({"role":"user", "content":[{"type":"tool_result","content":"missing id"}]}),
        json!({"role":"user", "content":[{"type":"tool_result","tool_use_id":"","content":"missing id"}]}),
    ] {
        assert!(async_launch_tool_results(&message).is_none());
        assert!(pure_async_launch_tool_results(&message).is_none());
    }
    let multi_text = request(json!([{
        "type":"tool_result", "tool_use_id":"one",
        "content":[{"type":"text", "text":ASYNC_LAUNCH_PREFIX}, {"type":"text", "text":BACKGROUND_MARKER}]
    }]));
    assert_eq!(
        async_launch_tool_results(multi_text.messages.last().unwrap()),
        Some(vec!["one".to_owned()])
    );
    let message = multi_text.messages.last().unwrap();
    assert!(exact_async_launch_acknowledgement(message, &[]).is_none());
    assert!(exact_async_launch_acknowledgement(message, &["different".to_owned()]).is_none());
    let expected = vec!["one".to_owned()];
    assert_eq!(
        exact_async_launch_acknowledgement(message, &expected),
        Some(expected)
    );
}

#[test]
fn validates_tool_round_ids_and_latest_assistant_round() {
    for message in [
        json!({"content":[]}),
        json!({"content":"not an array"}),
        json!({"content":[{"type":"tool_use", "id":""}]}),
        json!({"content":[{"type":"tool_use"}]}),
        json!({"content":[{"type":"text", "text":"no tools"}]}),
    ] {
        assert!(tool_round_ids(&message).is_none());
        assert!(agent_tool_round_ids(&message).is_none());
    }
    let mut current = request(json!([{"type":"tool_result", "tool_use_id":"one"}]));
    current.messages.insert(
        0,
        json!({"role":"assistant", "content":[{"type":"text","text":"no tools"}]}),
    );
    assert!(latest_tool_round_ids(&current).is_none());
    assert!(latest_agent_tool_round_ids(&current).is_none());
}

#[test]
fn exhausts_async_acknowledgement_and_text_shape_boundaries() {
    for message in [
        json!({"role":"user", "content":[{"type":"tool_result", "tool_use_id":"x", "content":42}]}),
        json!({"role":"user", "content":[{"type":"tool_result", "tool_use_id":"x", "content":"launch"}]}),
        json!({"role":"user", "content":[{"type":"tool_result", "tool_use_id":"x", "content":ASYNC_LAUNCH_PREFIX}]}),
        json!({"role":"user", "content":[{"type":"tool_result", "tool_use_id":"x", "content":"Async agent launched successfully.\nnot background"}]}),
        json!({"role":"user", "content":[{"type":"tool_result", "tool_use_id":"x", "is_error":true, "content":format!("{ASYNC_LAUNCH_PREFIX}\n{BACKGROUND_MARKER}")}]}),
    ] {
        assert!(async_launch_tool_results(&message).is_none());
        assert!(pure_async_launch_tool_results(&message).is_none());
    }
    let multiline = json!({
        "role":"user",
        "content":[{"type":"tool_result", "tool_use_id":"x", "content":[
            {"type":"text", "text":ASYNC_LAUNCH_PREFIX},
            {"type":"text", "text":BACKGROUND_MARKER}
        ]}]
    });
    assert_eq!(
        async_launch_tool_results(&multiline),
        Some(vec!["x".to_owned()])
    );
    assert!(
        exact_async_launch_acknowledgement(&multiline, &["x".to_owned(), "x".to_owned()]).is_none()
    );
    assert!(exact_async_launch_acknowledgement(&multiline, &["different".to_owned()]).is_none());
    let duplicate_expected = json!({
        "role":"user",
        "content":[
            {"type":"tool_result", "tool_use_id":"x", "content":[
                {"type":"text", "text":ASYNC_LAUNCH_PREFIX},
                {"type":"text", "text":BACKGROUND_MARKER}
            ]},
            {"type":"tool_result", "tool_use_id":"y", "content":[
                {"type":"text", "text":ASYNC_LAUNCH_PREFIX},
                {"type":"text", "text":BACKGROUND_MARKER}
            ]}
        ]
    });
    assert!(
        exact_async_launch_acknowledgement(&duplicate_expected, &["x".to_owned(), "x".to_owned()])
            .is_none()
    );
    assert_eq!(
        exact_async_launch_acknowledgement(&multiline, &["x".to_owned()]),
        Some(vec!["x".to_owned()])
    );
}

#[test]
fn invalid_text_items_are_rejected_without_status_generation() {
    assert!(append_strict_result_text(&mut String::new(), &json!({"type":"image"})).is_none());
}

#[test]
fn accepts_successful_async_launch_results_and_ignores_mixed_noise() {
    let pure = request(json!([launch_result("one"), launch_result("two")]));
    assert_eq!(
        async_launch_tool_results(pure.messages.last().unwrap()),
        Some(vec!["one".to_owned(), "two".to_owned()])
    );
    assert_eq!(
        pure_async_launch_tool_results(pure.messages.last().unwrap()),
        Some(vec!["one".to_owned(), "two".to_owned()])
    );

    let mixed_text = request(json!([launch_result("one"), {"type":"text", "text":"hi"}]));
    assert!(pure_async_launch_tool_results(mixed_text.messages.last().unwrap()).is_none());
    assert_eq!(
        async_launch_tool_results(mixed_text.messages.last().unwrap()),
        Some(vec!["one".to_owned()])
    );
    let failed = request(json!([{
        "type":"tool_result", "tool_use_id":"one", "is_error":true,
        "content":format!("{ASYNC_LAUNCH_PREFIX} {BACKGROUND_MARKER}")
    }]));
    assert!(async_launch_tool_results(failed.messages.last().unwrap()).is_none());
    let completed = request(json!([{
        "type":"tool_result", "tool_use_id":"one", "content":"finished"
    }]));
    assert!(async_launch_tool_results(completed.messages.last().unwrap()).is_none());
    let rich = request(json!([{
        "type":"tool_result", "tool_use_id":"one",
        "content":[{"type":"image"}, {"type":"text", "text":format!("{ASYNC_LAUNCH_PREFIX} {BACKGROUND_MARKER}")}]
    }]));
    assert!(async_launch_tool_results(rich.messages.last().unwrap()).is_none());
}

#[test]
fn hands_off_agent_results_even_when_the_latest_round_also_had_other_tools() {
    let mut mixed = request(json!([
        launch_result("background"),
        {
            "type":"tool_result",
            "tool_use_id":"other",
            "content":"file contents"
        }
    ]));
    mixed.messages.insert(
        0,
        json!({
            "role":"assistant",
            "content":[
                {"type":"text", "text":"Launching delegated work."},
                {"type":"tool_use", "id":"background", "name":"Agent", "input":{}},
                {"type":"tool_use", "id":"other", "name":"Read", "input":{}}
            ]
        }),
    );
    assert_eq!(
        latest_agent_tool_round_ids(&mixed),
        Some(vec!["background".to_owned()])
    );
    assert_eq!(
        exact_async_launch_acknowledgement(
            mixed.messages.last().unwrap(),
            &["background".to_owned()]
        ),
        Some(vec!["background".to_owned()])
    );
}

#[test]
fn requires_results_to_belong_to_the_latest_native_tool_round() {
    let mut correlated = request(json!([launch_result("background")]));
    correlated.messages.insert(
        0,
        json!({
            "role":"assistant",
            "content":[
                {"type":"text", "text":"Launching delegated work."},
                {"type":"tool_use", "id":"background", "name":"Agent", "input":{}},
                {"type":"tool_use", "id":"other", "name":"Read", "input":{}}
            ]
        }),
    );
    assert_eq!(
        latest_tool_round_ids(&correlated),
        Some(vec!["background".to_owned(), "other".to_owned()])
    );

    let uncorrelated = request(json!([launch_result("background")]));
    assert!(latest_tool_round_ids(&uncorrelated).is_none());
}

#[test]
fn requires_an_exact_unique_async_result_set() {
    let expected = vec!["one".to_owned(), "two".to_owned()];
    let exact = request(json!([launch_result("two"), launch_result("one")]));
    assert_eq!(
        exact_async_launch_acknowledgement(exact.messages.last().unwrap(), &expected),
        Some(vec!["two".to_owned(), "one".to_owned()])
    );

    let partial = request(json!([launch_result("one")]));
    assert!(
        exact_async_launch_acknowledgement(partial.messages.last().unwrap(), &expected).is_none()
    );
    let duplicate = request(json!([launch_result("one"), launch_result("one")]));
    assert!(
        exact_async_launch_acknowledgement(duplicate.messages.last().unwrap(), &expected).is_none()
    );
}

#[tokio::test]
async fn background_handoff_returns_visible_native_end_turn_without_lifecycle_tags() {
    let json_request = request(json!([launch_result("one")]));
    let response = internal_notification::acknowledge_with_text(
        &json_request,
        "Background agent launched; the main prompt is ready.",
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["stop_reason"], "end_turn");
    assert_eq!(
        body["content"][0]["text"],
        "Background agent launched; the main prompt is ready."
    );
    assert!(!body.to_string().contains("agent-message"));

    let mut stream_request = json_request;
    stream_request.stream = true;
    let response = internal_notification::acknowledge_with_text(
        &stream_request,
        "Background agent launched; the main prompt is ready.",
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains("event: message_start"));
    assert!(body.contains("content_block_delta"));
    assert!(body.contains("main prompt is ready"));
    assert!(body.contains(r#""stop_reason":"end_turn""#));
    assert!(body.contains("event: message_stop"));
}

#[test]
fn background_handoff_text_matches_the_native_launch_count() {
    assert_eq!(
        background_handoff_text(1),
        "Background agent launched; the main prompt is ready."
    );
    assert_eq!(
        background_handoff_text(3),
        "3 background agents launched; the main prompt is ready."
    );
}

#[test]
fn steering_shape_predicate_rejects_empty_and_non_text_content() {
    assert!(is_text_only_user_message(
        &json!({"role":"user", "content":"continue the audit"})
    ));
    assert!(!is_text_only_user_message(
        &json!({"role":"user", "content":"   "})
    ));
    assert!(!is_text_only_user_message(&json!({"role":"user"})));
    assert!(!is_text_only_user_message(
        &json!({"role":"user", "content":null})
    ));
}

#[test]
fn async_launch_results_ignore_empty_array_content() {
    assert!(
        async_launch_tool_results(&json!({
            "role":"user",
            "content":[{"type":"tool_result","tool_use_id":"x","content":[]}]
        }))
        .is_none()
    );
    assert!(strict_result_text(&json!([])).is_none());
}

#[test]
fn async_launch_results_skip_blank_tool_use_ids() {
    assert!(
        async_launch_tool_results(&json!({
            "role":"user",
            "content":[launch_result("")]
        }))
        .is_none()
    );
}

fn handoff_session(pending: HashMap<String, Value>) -> Arc<super::super::Session> {
    handoff_session_with("handoff-thread", pending, HashSet::new())
}

fn handoff_session_with(
    thread_id: &str,
    pending: HashMap<String, Value>,
    consumed: HashSet<String>,
) -> Arc<super::super::Session> {
    let slots = Arc::new(Semaphore::new(1));
    Arc::new(super::super::Session {
        thread_id: thread_id.to_owned(),
        model: "main-model".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("handoff-signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(pending),
        consumed_tool_ids: Mutex::new(consumed),
        external_tool_names: HashMap::new(),
        launch_availability: Default::default(),
        client_user_id: None,
        claude_session_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: slots.try_acquire_owned().expect("session slot"),
    })
}

fn background_agent_request(agent_id: &str) -> MessagesRequest {
    let mut request = request(json!([launch_result(agent_id)]));
    request.messages.insert(
        0,
        json!({
            "role":"assistant",
            "content":[
                {"type":"text", "text":"Launching background work."},
                {"type":"tool_use", "id":agent_id, "name":"Agent", "input":{}}
            ]
        }),
    );
    request
}

#[tokio::test]
async fn keeps_provider_open_when_non_async_tools_remain_pending() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main-model".to_owned());
    let mut pending = HashMap::new();
    pending.insert("background".to_owned(), json!(1));
    pending.insert("bash-1".to_owned(), json!(2));
    bridge.sessions.lock().await.push(handoff_session(pending));

    let response = bridge
        .async_agent_launch_handoff(&background_agent_request("background"))
        .await;
    assert!(
        response.is_none(),
        "leftover pending tools must keep the provider turn open"
    );
}

#[tokio::test]
async fn hands_control_back_when_no_session_owns_the_async_results() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main-model".to_owned());
    let response = bridge
        .async_agent_launch_handoff(&background_agent_request("background"))
        .await
        .expect("unowned async launch acknowledgement still hands control back");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["stop_reason"], "end_turn");
    assert!(
        body["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("Background agent launched")
    );
}

#[tokio::test]
async fn hands_control_back_when_steering_user_message_follows_async_ack() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main-model".to_owned());
    let mut request = background_agent_request("background");
    request.messages.push(json!({
        "role":"user",
        "content":[{
            "type":"text",
            "text":"The user sent a new message while you were working:\n優先して調査\n\nAddress the message above as you continue this turn."
        }]
    }));
    let response = bridge
        .async_agent_launch_handoff(&request)
        .await
        .expect("async ack before trailing steering must still hand control back");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["stop_reason"], "end_turn");
}

#[tokio::test]
async fn keeps_provider_open_for_a_partial_async_launch_ack() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main-model".to_owned());
    let mut request = request(json!([launch_result("one")]));
    request.messages.insert(
        0,
        json!({
            "role":"assistant",
            "content":[
                {"type":"tool_use", "id":"one", "name":"Agent", "input":{}},
                {"type":"tool_use", "id":"two", "name":"Agent", "input":{}}
            ]
        }),
    );

    assert!(
        bridge.async_agent_launch_handoff(&request).await.is_none(),
        "a partial acknowledgement must not hand control back"
    );
}

#[tokio::test]
async fn keeps_provider_open_for_an_empty_async_launch_round() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main-model".to_owned());
    let request = request(json!([]));

    assert!(
        bridge.async_agent_launch_handoff(&request).await.is_none(),
        "an empty launch round must not hand control back"
    );
}

#[tokio::test]
async fn keeps_provider_open_when_thread_ensure_fails() {
    let backend = AgentBackend::spawn_routes(&[BackendRoute {
        model: "failed".to_owned(),
        backend: BackendKind::ConfiguredAcp,
        effort: None,
        model_provider: None,
        model_catalog_json: None,
        pi_provider: None,
        pi_model: None,
        max_context_tokens: None,
        max_concurrency: None,
        model_prefixes: Vec::new(),
        acp: Some(AcpLaunch {
            program: "/definitely/missing/claudex-acp".to_owned(),
            arguments: Vec::new(),
        }),
        web_search_mode: WebSearchMode::default(),
    }]);
    let bridge = Bridge::new_with_backend(backend, "failed".to_owned());
    let mut consumed = HashSet::new();
    consumed.insert("background".to_owned());
    bridge.sessions.lock().await.push(handoff_session_with(
        "0:missing-thread",
        HashMap::new(),
        consumed,
    ));

    let response = bridge
        .async_agent_launch_handoff(&background_agent_request("background"))
        .await;
    assert!(
        response.is_none(),
        "handoff must abort when the owning thread cannot be ensured"
    );
}

fn parallel_background_agent_request(agent_id: &str) -> MessagesRequest {
    let mut request = background_agent_request(agent_id);
    request.messages.insert(
        0,
        json!({
            "role":"user",
            "content":"並列で3つの独立スコープを実装して調査と実装を分担してください。"
        }),
    );
    request
}

fn scheduler_handoff_request(user_text: &str) -> MessagesRequest {
    let mut request = background_agent_request("background");
    request.messages.insert(
        0,
        json!({
            "role":"user",
            "content":user_text
        }),
    );
    request
}

#[test]
fn exact_two_scope_target_hands_off_at_two() {
    let scheduler = ParallelScheduler::for_tests();
    let request = scheduler_handoff_request(
        "Implement exactly these 2 independent scopes:\n- implement parser\n- verify renderer",
    );

    assert!(should_defer_background_handoff_with(
        &scheduler, &request, 1
    ));
    assert!(!should_defer_background_handoff_with(
        &scheduler, &request, 2
    ));
}

#[test]
fn single_scope_substantive_work_hands_off_at_one_worker() {
    let scheduler = ParallelScheduler::new(SchedulerConfig {
        min_parallel_workers: 5,
        max_parallel_workers: 8,
        ..SchedulerConfig::default()
    });
    let request = scheduler_handoff_request("Implement the authentication cache.");

    assert!(should_defer_background_handoff_with(
        &scheduler, &request, 0
    ));
    assert!(!should_defer_background_handoff_with(
        &scheduler, &request, 1
    ));
}

#[test]
fn seven_scopes_respect_max_four_before_handoff() {
    let scheduler = ParallelScheduler::new(SchedulerConfig {
        max_parallel_workers: 4,
        ..SchedulerConfig::default()
    });
    let request = scheduler_handoff_request(
        "Tasks:\n- implement one\n- implement two\n- implement three\n- implement four\n- implement five\n- implement six\n- implement seven",
    );

    assert!(should_defer_background_handoff_with(
        &scheduler, &request, 3
    ));
    assert!(!should_defer_background_handoff_with(
        &scheduler, &request, 4
    ));
}

#[test]
fn no_delegation_intent_never_defers_handoff() {
    let scheduler = ParallelScheduler::for_tests();
    let request = scheduler_handoff_request("Do not launch another SubAgent.");

    assert!(!should_defer_background_handoff_with(
        &scheduler, &request, 0
    ));
}

#[tokio::test]
async fn defers_handoff_when_parallel_floor_is_unmet() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main-model".to_owned());
    let response = bridge
        .async_agent_launch_handoff(&parallel_background_agent_request("background"))
        .await;
    assert!(
        response.is_none(),
        "partial fan-out must keep the provider turn open for additional launches"
    );
}

#[tokio::test]
async fn hands_control_back_once_parallel_floor_is_met() {
    let bridge = Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main-model".to_owned());
    let config = crate::parallel_scheduler::ParallelScheduler::shared().config();
    let required = config.min_parallel_workers.max(config.active_floor);
    let ids = (0..required)
        .map(|index| format!("worker-{index}"))
        .collect::<Vec<_>>();
    let mut request = request(json!(
        ids.iter().map(|id| launch_result(id)).collect::<Vec<_>>()
    ));
    request.messages.insert(
        0,
        json!({
            "role":"user",
            "content":"並列で複数の独立スコープを実装してください。"
        }),
    );
    request.messages.insert(
        1,
        json!({
            "role":"assistant",
            "content": ids.iter().map(|id| json!({
                "type":"tool_use",
                "id": id,
                "name":"Agent",
                "input":{}
            })).collect::<Vec<_>>()
        }),
    );
    let response = bridge
        .async_agent_launch_handoff(&request)
        .await
        .expect("met floor should hand control back");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["stop_reason"], "end_turn");
}
