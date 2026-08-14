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
    anthropic::{
        ContextRetry, MessagesRequest, Segment, Session, Usage, WebEvidenceSummary,
        model_concurrency::ModelPermit,
    },
    app_server::AppServer,
};

async fn run_attached_subagent(bridge: Arc<Bridge>, turn: ActiveTurn) -> Result<Response<Body>> {
    bridge.non_streaming_subagent_response(turn, None).await
}

async fn run_subagent_with_timeout(
    bridge: Arc<Bridge>,
    turn: ActiveTurn,
    timeout: Duration,
) -> Result<Response<Body>> {
    bridge
        .non_streaming_subagent_response_with_timeout(turn, None, Some(timeout))
        .await
}

async fn settle_with_abort_barrier(
    bridge: Arc<Bridge>,
    turn: ActiveTurn,
    model_permit: ModelPermit,
    abort_started: tokio::sync::oneshot::Sender<()>,
    abort_release: tokio::sync::oneshot::Receiver<()>,
) {
    let _model_permit = model_permit;
    bridge
        .settle_expired_provider_with(
            &turn,
            Err(anyhow!("provider cancellation did not settle")),
            || async move {
                let _ = abort_started.send(());
                abort_release.await.expect("release provider abort");
                Ok(())
            },
        )
        .await;
}

#[test]
fn hard_timeout_is_opt_in_and_accepts_the_legacy_alias() {
    assert_eq!(subagent_hard_timeout_from(|_| None), None);
    assert_eq!(
        subagent_hard_timeout_from(|name| {
            (name == SUBAGENT_HARD_TIMEOUT_ENV).then(|| "7".to_owned())
        }),
        Some(Duration::from_secs(7))
    );
    assert_eq!(
        subagent_hard_timeout_from(|name| {
            (name == LEGACY_SUBAGENT_RESPONSE_TIMEOUT_ENV).then(|| "9".to_owned())
        }),
        Some(Duration::from_secs(9))
    );
    assert_eq!(subagent_hard_timeout_from(|_| Some("0".to_owned())), None);
    assert_eq!(
        subagent_hard_timeout_from(|_| Some("not-a-duration".to_owned())),
        None
    );
}

#[tokio::test]
async fn distinguishes_completed_and_backgrounded_work() {
    assert_eq!(
        completes_within(Some(Duration::from_secs(1)), async { 7 }).await,
        Some(7)
    );
    assert_eq!(
        completes_within(Some(Duration::ZERO), std::future::pending::<u8>()).await,
        None
    );
    assert_eq!(completes_within(None, async { 9 }).await, Some(9));
}

#[tokio::test]
async fn bounds_a_provider_cancellation_that_never_settles() {
    tokio::time::pause();
    let cancellation = tokio::spawn(provider_cancellation_within(
        std::future::pending(),
        Duration::from_secs(5),
    ));
    tokio::task::yield_now().await;
    assert!(!cancellation.is_finished());

    tokio::time::advance(Duration::from_secs(5)).await;
    let error = cancellation
        .await
        .expect("provider cancellation task")
        .expect_err("pending cancellation must be bounded");
    assert!(
        error
            .to_string()
            .contains("did not settle within 5 seconds")
    );
}

#[tokio::test]
async fn unset_hard_timeout_stays_attached_beyond_300_seconds() {
    let (_root, bridge) = mock_bridge(STALLED_APP_SERVER).await;
    tokio::time::pause();
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let turn = active_turn(dispatcher.subscribe("thread"), None).await;
    let session = Arc::clone(&turn.session);
    bridge.sessions.lock().await.push(Arc::clone(&session));

    let task = tokio::spawn(run_attached_subagent(Arc::clone(&bridge), turn));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(301)).await;
    tokio::task::yield_now().await;

    assert!(
        !task.is_finished(),
        "unset timeout ended the native Agent turn"
    );
    assert!(bridge.detached_sessions.lock().await.is_empty());
    assert!(
        bridge
            .sessions
            .lock()
            .await
            .iter()
            .any(|active| Arc::ptr_eq(active, &session))
    );

    dispatcher.dispatch(json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":"thread","delta":"completed after 301 seconds"}
    }));
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));

    let response = task
        .await
        .expect("native turn task")
        .expect("native turn response");
    assert!(
        response_text(response)
            .await
            .contains("completed after 301 seconds")
    );
}

#[tokio::test]
async fn explicit_hard_timeout_cancels_without_detaching_or_processing_late_events() {
    let (_root, bridge) = mock_bridge(STALLED_APP_SERVER).await;
    tokio::time::pause();
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let turn = active_turn(dispatcher.subscribe("thread"), None).await;
    let session = Arc::clone(&turn.session);
    bridge.sessions.lock().await.push(Arc::clone(&session));

    let task = tokio::spawn(run_subagent_with_timeout(
        Arc::clone(&bridge),
        turn,
        Duration::from_secs(5),
    ));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::task::yield_now().await;

    let error = task
        .await
        .expect("hard-timeout task")
        .expect_err("hard timeout must fail visibly");
    assert!(
        error
            .to_string()
            .contains("configured hard timeout of 5 seconds")
    );
    assert!(bridge.sessions.lock().await.is_empty());
    assert!(bridge.detached_sessions.lock().await.is_empty());
    assert!(
        bridge.app.is_alive(),
        "hard timeout must clear the session without killing the shared Codex app-server"
    );
    assert_eq!(
        bridge
            .subagent_hard_timeout_cancel_attempts
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    dispatcher.dispatch(json!({
        "method":"item/agentMessage/delta",
        "params":{"threadId":"thread","delta":"must not be processed"}
    }));
    dispatcher.dispatch(json!({
        "method":"item/tool/call",
        "params":{
            "threadId":"thread",
            "item":{"id":"late-tool","name":"Read","arguments":{"path":"ignored"}}
        }
    }));
    dispatcher.dispatch(json!({
        "method":"turn/completed",
        "params":{"threadId":"thread","turn":{"status":"completed"}}
    }));
    tokio::task::yield_now().await;
    assert!(session.transcript.lock().await.is_empty());
    assert!(session.pending_tools.lock().await.is_empty());
}

#[tokio::test]
async fn failed_cancel_settlement_aborts_the_provider_before_returning() {
    let (_root, bridge) = mock_bridge(STALLED_APP_SERVER).await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let turn = active_turn(dispatcher.subscribe("thread"), None).await;
    bridge.sessions.lock().await.push(Arc::clone(&turn.session));

    bridge
        .settle_expired_provider(&turn, Err(anyhow!("ACP cancellation did not settle")))
        .await;

    assert!(bridge.sessions.lock().await.is_empty());
    assert!(bridge.detached_sessions.lock().await.is_empty());
    assert!(
        bridge.app.is_alive(),
        "failed cancel settlement must drop the session without killing the shared Codex app-server"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn settled_expiration_retires_the_session_without_aborting_the_provider() {
    let (_root, bridge) = mock_bridge(STALLED_APP_SERVER).await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let turn = active_turn(dispatcher.subscribe("thread"), None).await;
    bridge.sessions.lock().await.push(Arc::clone(&turn.session));
    let abort_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let abort_called_by_provider = Arc::clone(&abort_called);
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(std::io::sink)
        .finish();
    let _subscriber_guard = tracing::subscriber::set_default(subscriber);

    bridge
        .settle_expired_provider_with(
            &turn,
            Ok(crate::agent_backend::TurnCancellation::Settled),
            || async move {
                abort_called_by_provider.store(true, std::sync::atomic::Ordering::Relaxed);
                Ok(())
            },
        )
        .await;

    assert!(bridge.sessions.lock().await.is_empty());
    assert!(!abort_called.load(std::sync::atomic::Ordering::Relaxed));
    assert!(bridge.app.is_alive());
}

#[tokio::test]
async fn failed_provider_abort_shuts_down_the_shared_backend() {
    let (_root, bridge) = mock_bridge(STALLED_APP_SERVER).await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let turn = active_turn(dispatcher.subscribe("thread"), None).await;
    bridge.sessions.lock().await.push(Arc::clone(&turn.session));

    bridge
        .settle_expired_provider_with(
            &turn,
            Ok(crate::agent_backend::TurnCancellation::Unsupported),
            || async { Err(anyhow!("targeted provider abort failed")) },
        )
        .await;

    assert!(bridge.sessions.lock().await.is_empty());
    assert!(bridge.detached_sessions.lock().await.is_empty());
    assert!(
        !bridge.app.is_alive(),
        "failed targeted abort must shut down the shared app-server"
    );
}

#[tokio::test]
async fn provider_abort_completion_precedes_model_and_session_permit_release() {
    let (_root, bridge) = mock_bridge(STALLED_APP_SERVER).await;
    let dispatcher = crate::app_server::events::ThreadEventDispatcher::default();
    let (turn, session_slots) = active_turn_with_slots(dispatcher.subscribe("thread"), None).await;
    bridge.sessions.lock().await.push(Arc::clone(&turn.session));
    let model_permit = bridge
        .model_concurrency
        .ticket("bounded-provider", Some(1))
        .expect("configured model ticket")
        .acquire()
        .await
        .expect("model permit");
    let (abort_started_tx, abort_started_rx) = tokio::sync::oneshot::channel();
    let (abort_release_tx, abort_release_rx) = tokio::sync::oneshot::channel();

    let task = tokio::spawn(settle_with_abort_barrier(
        Arc::clone(&bridge),
        turn,
        model_permit,
        abort_started_tx,
        abort_release_rx,
    ));
    abort_started_rx.await.expect("provider abort started");

    assert!(
        !task.is_finished(),
        "expiration returned before provider abort"
    );
    assert_eq!(session_slots.available_permits(), 0);
    assert_eq!(model_active_permits(&bridge, "bounded-provider"), 1);
    assert!(bridge.sessions.lock().await.is_empty());

    abort_release_tx.send(()).expect("finish provider abort");
    task.await.expect("expiration task");
    assert_eq!(session_slots.available_permits(), 1);
    assert_eq!(model_active_permits(&bridge, "bounded-provider"), 0);
}

fn model_active_permits(bridge: &Bridge, model: &str) -> u64 {
    serde_json::to_value(bridge.model_concurrency.snapshot()).expect("serialize model status")
        [model]["active"]
        .as_u64()
        .expect("active model permit count")
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

fn completed_segment() -> Segment {
    Segment {
        blocks: vec![json!({"type":"text","text":"completed late result"})],
        stop_reason: "end_turn",
        usage: Usage {
            input_tokens: 1,
            output_tokens: 1,
            web_search_requests: 0,
        },
        web_evidence: WebEvidenceSummary::default(),
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
            Some(Duration::from_secs(1)),
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
            Some(Duration::from_secs(1)),
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
    let bridge = Bridge::new_with_subscription_program(
        app,
        "main".to_owned(),
        root.path().join("unused-claude"),
    )
    .with_subagent_hard_timeout(Some(Duration::from_secs(11)));
    assert_eq!(bridge.subagent_hard_timeout_seconds(), Some(11));
    let bridge = bridge.with_subagent_hard_timeout(None);
    assert_eq!(bridge.subagent_hard_timeout_seconds(), None);
    (root, Arc::new(bridge))
}

async fn active_turn(
    events: crate::app_server::ThreadEvents,
    retry: Option<ContextRetry>,
) -> ActiveTurn {
    active_turn_with_slots(events, retry).await.0
}

async fn active_turn_with_slots(
    events: crate::app_server::ThreadEvents,
    retry: Option<ContextRetry>,
) -> (ActiveTurn, Arc<Semaphore>) {
    let slots = Arc::new(Semaphore::new(1));
    let session = Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(HashSet::new()),
        external_tool_names: HashMap::new(),
        launch_availability: Default::default(),
        client_user_id: None,
        claude_session_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: Arc::clone(&slots)
            .try_acquire_owned()
            .expect("session slot"),
    });
    let gate = Arc::clone(&session.gate).lock_owned().await;
    let turn = ActiveTurn {
        session,
        events: Arc::new(events),
        response_model: "main".to_owned(),
        extras: Vec::new(),
        routing_system: Value::Null,
        input_tokens: 1,
        retry,
        gate,
        detached: false,
    };
    (turn, slots)
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
