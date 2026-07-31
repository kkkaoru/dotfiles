use std::{
    collections::{HashMap, HashSet},
    os::unix::fs::PermissionsExt,
    sync::Arc,
    time::Instant,
};

use axum::body::to_bytes;
use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore};

use super::*;
use crate::{
    anthropic::{ContextRetry, MessagesRequest, Session},
    app_server::AppServer,
};

#[test]
fn defaults_and_validates_the_subagent_response_timeout() {
    assert_eq!(
        subagent_response_timeout_from(|_| None),
        Duration::from_secs(300)
    );
    assert_eq!(
        subagent_response_timeout_from(|_| Some("7".to_owned())),
        Duration::from_secs(7)
    );
    assert_eq!(
        subagent_response_timeout_from(|_| Some("0".to_owned())),
        Duration::from_secs(300)
    );
    assert_eq!(
        subagent_response_timeout_from(|_| Some("not-a-duration".to_owned())),
        Duration::from_secs(300)
    );
}

#[tokio::test]
async fn distinguishes_completed_and_backgrounded_work() {
    assert_eq!(
        completes_within(Duration::from_secs(1), async { 7 }).await,
        Some(7)
    );
    assert_eq!(
        completes_within(Duration::ZERO, std::future::pending::<u8>()).await,
        None
    );
}

#[tokio::test]
async fn background_progress_falls_back_for_unconfigured_empty_and_failed_models() {
    let (root, bridge) = mock_bridge(STALLED_APP_SERVER).await;
    let settings = root.path().join("settings.json");
    std::fs::write(&settings, "{}").expect("remove progress model setting");
    assert_eq!(
        bridge
            .background_progress_text(&progress_turn().await)
            .await,
        BACKGROUND_PROGRESS_FALLBACK
    );

    std::fs::write(&settings, r#"{"model":"mock-progress"}"#).expect("restore model setting");
    write_progress_model(
        &root,
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"\"}'\n",
    );
    assert_eq!(
        bridge
            .background_progress_text(&progress_turn().await)
            .await,
        BACKGROUND_PROGRESS_FALLBACK
    );

    write_progress_model(
        &root,
        "#!/bin/sh\ncat >/dev/null\nprintf failed >&2\nexit 1\n",
    );
    assert_eq!(
        bridge
            .background_progress_text(&progress_turn().await)
            .await,
        BACKGROUND_PROGRESS_FALLBACK
    );
}

#[tokio::test]
async fn background_progress_prompt_truncates_oversized_unicode_context() {
    let mut turn = progress_turn().await;
    let tail = "retain this tail";
    turn.extras = vec![json!({"context": format!("{}{}", "🦀".repeat(16_001), tail)})];

    let prompt = background_progress_prompt(&turn).await;

    assert!(prompt.contains("[earlier context truncated]"));
    assert!(prompt.contains(tail));
}

#[tokio::test]
async fn background_timeout_preserves_late_result_and_releases_detached_session() {
    let (_root, bridge) = mock_bridge(STALLED_APP_SERVER).await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let turn = active_turn(dispatcher.subscribe("thread"), None).await;
    let session = Arc::clone(&turn.session);

    let response = bridge
        .non_streaming_subagent_response_with_timeout(turn, None, Duration::ZERO)
        .await
        .expect("background response");
    assert!(
        response_text(response)
            .await
            .contains("dynamic progress from progress subagent")
    );

    dispatcher.dispatch(json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":"thread","delta":"late result"}
    }));
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));

    tokio::time::timeout(Duration::from_secs(1), async {
        while !late_result_is_released(&session, &bridge).await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("late result should be committed and detached session released");
}

#[tokio::test]
async fn completed_detached_subagent_turn_commits_and_releases_its_session() {
    let (_root, bridge) = mock_bridge(STALLED_APP_SERVER).await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let mut turn = active_turn(dispatcher.subscribe("thread"), None).await;
    let session = Arc::clone(&turn.session);
    bridge.detach_session(&session).await;
    turn.detached = true;

    let response = completion::finish(&bridge, turn, completed_segment()).await;

    assert!(
        response_text(response)
            .await
            .contains("completed late result")
    );
    assert!(
        session
            .transcript
            .lock()
            .await
            .iter()
            .any(|entry| entry.to_string().contains("completed late result"))
    );
    assert!(bridge.detached_sessions.lock().await.is_empty());
}

async fn late_result_is_released(session: &Arc<Session>, bridge: &Arc<Bridge>) -> bool {
    let has_result = {
        let transcript = session.transcript.lock().await;
        transcript
            .iter()
            .any(|entry| entry.to_string().contains("late result"))
    };
    let detached_empty = bridge.detached_sessions.lock().await.is_empty();
    has_result && detached_empty
}

fn completed_segment() -> Segment {
    Segment {
        blocks: vec![json!({"type":"text","text":"completed late result"})],
        stop_reason: "end_turn",
        usage: Usage {
            input_tokens: 1,
            output_tokens: 1,
            web_search_requests: 0,
        },
    }
}

#[tokio::test]
async fn streams_a_subagent_response_without_waiting_for_the_provider_turn() {
    let (_root, bridge) = mock_bridge(STALLED_APP_SERVER).await;
    let mut request = retry().request;
    request.stream = true;

    let response = bridge
        .provider_messages(request, 1, None, true, true)
        .await
        .expect("streaming subagent response");

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn fails_a_context_window_subagent_turn_without_a_retry() {
    let (_root, bridge) = mock_bridge(STALLED_APP_SERVER).await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));

    let error = bridge
        .non_streaming_subagent_response_with_timeout(
            active_turn(events, None).await,
            None,
            Duration::from_secs(1),
        )
        .await
        .expect_err("unretryable context window error");

    assert!(error.to_string().contains("context window exceeded"));
}

#[tokio::test]
async fn completes_and_retries_a_non_streaming_subagent_turn() {
    let (_root, bridge) = mock_bridge(RETRYING_APP_SERVER).await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let events = dispatcher.subscribe("thread");
    dispatcher.dispatch(json!({
        "method":"error",
        "params":{"threadId":"thread","error":{"message":"context window exceeded"}}
    }));
    let response = bridge
        .non_streaming_subagent_response_with_timeout(
            active_turn(events, Some(retry())).await,
            None,
            Duration::from_secs(1),
        )
        .await
        .expect("retried subagent response");

    let body = response_text(response).await;
    assert!(body.contains("retried"), "unexpected response: {body}");
}

async fn mock_bridge(script: &str) -> (tempfile::TempDir, Arc<Bridge>) {
    let root = tempfile::tempdir().expect("subagent timeout fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("create source home");
    std::fs::write(source.join("auth.json"), "{}").expect("write source auth");
    let program = root.path().join("mock-app-server");
    std::fs::write(&program, script).expect("write app-server mock");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("make app-server mock executable");
    let app =
        AppServer::spawn_with_program("main", &program, &source, &root.path().join("isolated"))
            .await
            .expect("start app-server mock");
    let progress_program = root.path().join("mock-claude");
    std::fs::write(
            &progress_program,
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"result\":\"dynamic progress from progress subagent\"}'\n",
        )
        .expect("write progress model mock");
    std::fs::set_permissions(&progress_program, std::fs::Permissions::from_mode(0o755))
        .expect("make progress model mock executable");
    let settings = root.path().join("settings.json");
    std::fs::write(&settings, r#"{"model":"mock-progress"}"#)
        .expect("write progress model settings");
    let bridge = Bridge::new_with_subscription_program(app, "main".to_owned(), progress_program)
        .with_settings_path(settings);
    (root, Arc::new(bridge))
}

async fn active_turn(
    events: crate::app_server::ThreadEvents,
    retry: Option<ContextRetry>,
) -> ActiveTurn {
    let slots = Arc::new(Semaphore::new(1));
    let session = Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        internal_tools: HashMap::new(),
        external_tool_names: HashMap::new(),
        client_user_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        _slot: slots.try_acquire_owned().expect("session slot"),
    });
    let gate = Arc::clone(&session.gate).lock_owned().await;
    ActiveTurn {
        session,
        events: Arc::new(events),
        response_model: "main".to_owned(),
        extras: Vec::new(),
        routing_system: Value::Null,
        input_tokens: 1,
        retry,
        gate,
        detached: false,
    }
}

async fn progress_turn() -> ActiveTurn {
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    active_turn(dispatcher.subscribe("thread"), None).await
}

fn write_progress_model(root: &tempfile::TempDir, script: &str) {
    let program = root.path().join("mock-claude");
    std::fs::write(&program, script).expect("write progress model mock");
}

fn retry() -> ContextRetry {
    ContextRetry {
        request: MessagesRequest {
            model: "main".to_owned(),
            system: Value::Null,
            messages: vec![json!({"role":"user","content":"retry"})],
            tools: Vec::new(),
            stream: false,
            output_config: Value::Null,
            metadata: Value::Null,
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        },
        effort: None,
        advisor_model: None,
        collaborator_model: None,
    }
}

async fn response_text(response: Response<Body>) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    String::from_utf8(bytes.to_vec()).expect("UTF-8 response")
}

const STALLED_APP_SERVER: &str = "#!/bin/sh\nread initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nwhile read line; do :; done\n";
const RETRYING_APP_SERVER: &str = "#!/bin/sh\nread initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nread create\nprintf '%s\\n' '{\"id\":2,\"result\":{\"thread\":{\"id\":\"retried\"}}}'\nread turn\nprintf '%s\\n' '{\"method\":\"item/agentMessage/delta\",\"params\":{\"threadId\":\"retried\",\"delta\":\"retried\"}}'\nprintf '%s\\n' '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"retried\",\"turn\":{\"status\":\"completed\"}}}'\nwhile read line; do :; done\n";
