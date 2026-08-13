use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::json;
use tokio::sync::{Mutex, Semaphore};

use super::*;
use crate::{agent_backend::AgentBackend, app_server::AppServer};

#[tokio::test]
async fn skips_busy_codex_reuse_after_an_unsupported_cancellation() {
    let app = test_app().await;
    let session = session("main-model", Some("client"));
    let gate = Arc::clone(&session.gate).lock_owned().await;
    let request = request("");
    assert!(
        find_busy_matching_session(
            vec![Arc::clone(&session)],
            &Arc::from("signature"),
            &request.messages,
            Some(&request.model),
            Some("client"),
            None,
        )
        .await
        .is_some(),
        "fixture must be eligible for busy-session preemption"
    );
    let task = start_preemption(Arc::clone(&app), Arc::clone(&session), request);
    let selected = tokio::time::timeout(Duration::from_millis(100), task)
        .await
        .expect("unsupported cancellation must not wait on the busy gate")
        .expect("preemption task");
    assert!(selected.is_none());
    drop(gate);
    app.shutdown().await;
}

#[tokio::test]
async fn does_not_preempt_a_busy_first_turn_for_a_parallel_request() {
    let app = test_app().await;
    let busy = session("main-model", Some("client"));
    busy.transcript.lock().await.clear();
    let gate = Arc::clone(&busy.gate).lock_owned().await;
    let mut parallel = request("main-model");
    parallel.messages = vec![json!({"role":"user","content":"independent"})];
    assert!(
        select_matching_session(
            vec![busy],
            &parallel,
            &Arc::from("signature"),
            &parallel.messages,
            &app,
        )
        .await
        .is_none(),
        "parallel first turns must not cancel each other"
    );
    drop(gate);
    app.shutdown().await;
}

#[tokio::test]
async fn reuses_idle_sessions_and_preempts_matching_subagent_follow_ups() {
    let app = test_app().await;
    let request = request("main-model");
    let idle = session("main-model", Some("client"));
    let selected = select_matching_session(
        vec![Arc::clone(&idle)],
        &request,
        &Arc::from("signature"),
        &request.messages,
        &app,
    )
    .await
    .expect("idle session must be reused");
    assert!(Arc::ptr_eq(&selected.session, &idle));
    drop(selected);
    app.shutdown().await;

    let app = stopped_acp_app();
    let busy = session("main-model", Some("client"));
    let gate = Arc::clone(&busy.gate).lock_owned().await;
    let task = start_preemption(Arc::clone(&app), Arc::clone(&busy), subagent_request());
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(
        !task.is_finished(),
        "matching SubAgent follow-up must preempt instead of skipping"
    );
    drop(gate);
    let selected = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("subagent preemption must settle")
        .expect("preemption task")
        .expect("released busy subagent session");
    assert!(Arc::ptr_eq(&selected.session, &busy));
    drop(selected);
    app.shutdown().await;
}

#[tokio::test]
async fn does_not_preempt_parallel_subagents_with_different_signatures() {
    let app = stopped_acp_app();
    let busy = session_named("main-model", Some("client"), "agent-a");
    let gate = Arc::clone(&busy.gate).lock_owned().await;
    let subagent = subagent_request();
    let selected = tokio::time::timeout(
        Duration::from_millis(100),
        select_matching_session(
            vec![busy],
            &subagent,
            &Arc::from("agent-b"),
            &subagent.messages,
            &app,
        ),
    )
    .await
    .expect("parallel SubAgents must not wait on another worker's gate");
    assert!(selected.is_none());
    drop(gate);
    app.shutdown().await;
}

#[tokio::test]
async fn outer_follow_up_preempts_busy_session_after_signature_drift() {
    let app = stopped_acp_app();
    let busy = session_named("main-model", Some("client"), "drifted");
    let gate = Arc::clone(&busy.gate).lock_owned().await;
    let task = start_preemption(Arc::clone(&app), Arc::clone(&busy), request("main-model"));
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(
        !task.is_finished(),
        "outer follow-up must still preempt via model+user fallback"
    );
    drop(gate);
    let selected = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("outer drifted-signature preemption must settle")
        .expect("preemption task")
        .expect("released busy outer session");
    assert!(Arc::ptr_eq(&selected.session, &busy));
    drop(selected);
    app.shutdown().await;
}

#[test]
fn classifies_transport_identity_follow_ups_as_subagent_requests() {
    assert!(crate::anthropic::agent_effort::is_subagent_request(
        &subagent_request()
    ));
    assert!(!crate::anthropic::agent_effort::is_subagent_request(
        &request("main-model")
    ));
}

#[tokio::test]
async fn preempts_first_turn_subagent_follow_up_with_matching_signature() {
    let app = stopped_acp_app();
    let busy = session("main-model", Some("client"));
    busy.transcript.lock().await.clear();
    let gate = Arc::clone(&busy.gate).lock_owned().await;
    let task = start_preemption(Arc::clone(&app), Arc::clone(&busy), subagent_request());
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(
        !task.is_finished(),
        "first-turn SubAgent follow-up must still preempt"
    );
    drop(gate);
    let selected = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("first-turn subagent preemption must settle")
        .expect("preemption task")
        .expect("released first-turn subagent session");
    assert!(Arc::ptr_eq(&selected.session, &busy));
    drop(selected);
    app.shutdown().await;
}

#[tokio::test]
async fn reuses_a_busy_session_after_an_acp_cancellation_failure() {
    let app = stopped_acp_app();
    let session = session("main-model", Some("client"));
    let gate = Arc::clone(&session.gate).lock_owned().await;
    let request = request("main-model");
    let task = start_preemption(Arc::clone(&app), Arc::clone(&session), request);

    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(
        !task.is_finished(),
        "preemption must wait for the busy gate"
    );
    drop(gate);

    let selected = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("cancellation failure must not stall preemption")
        .expect("preemption task")
        .expect("released busy session");
    assert!(Arc::ptr_eq(&selected.session, &session));
    drop(selected);
    app.shutdown().await;
}

#[tokio::test]
async fn stale_busy_gate_holds_do_not_block_preemption_forever() {
    let request = request("main-model");
    let session = session("main-model", Some("client"));
    let _hold = Arc::clone(&session.gate).lock_owned().await;
    let messages = request.messages.clone();

    let selection = tokio::spawn(wait_for_preemption(Arc::clone(&session), messages.clone()));

    // PREEMPT_GATE_TIMEOUT (500ms) fires inside take_gate_after_preempt; wait
    // just past it rather than an arbitrary multi-second margin.
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let selection = selection
        .await
        .expect("take_gate_after_preempt must complete");
    drop(_hold);
    assert!(selection.is_none());
}

async fn wait_for_preemption(
    session: Arc<Session>,
    messages: Vec<Value>,
) -> Option<SelectedSession> {
    take_gate_after_preempt(&session, &messages, false).await
}

#[tokio::test]
async fn pending_busy_tools_prevent_session_reuse_after_failed_cancel() {
    let session = session("main-model", Some("client"));
    session
        .pending_tools
        .lock()
        .await
        .insert("tool-1".to_owned(), serde_json::json!({"id":"tool-1"}));
    assert!(
        take_gate_after_preempt(&session, &[json!({"role":"user","content":"first"})], false)
            .await
            .is_none(),
        "failed cancel must not reuse a session that still owns pending tools"
    );
}

#[tokio::test]
async fn settled_preempt_clears_pending_tools_for_pure_mid_turn() {
    let session = session("main-model", Some("client"));
    session
        .pending_tools
        .lock()
        .await
        .insert("tool-1".to_owned(), serde_json::json!({"id":"tool-1"}));
    let selected =
        take_gate_after_preempt(&session, &[json!({"role":"user","content":"first"})], true)
            .await
            .expect("settled cancel may abandon pending tools for the follow-up");
    assert!(Arc::ptr_eq(&selected.session, &session));
    assert!(selected.session.pending_tools.lock().await.is_empty());
}

#[tokio::test]
async fn dead_driver_settle_clears_pending_tools_and_reuses_for_follow_up() {
    let app = stopped_acp_app();
    let session = session("main-model", Some("client"));
    let gate = Arc::clone(&session.gate).lock_owned().await;
    let request = request("main-model");
    let task = start_preemption(Arc::clone(&app), Arc::clone(&session), request);

    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(
        !task.is_finished(),
        "preemption must wait for the active turn before settling pending tools"
    );
    session
        .pending_tools
        .lock()
        .await
        .insert("tool-1".to_owned(), json!({"id":"tool-1"}));
    drop(gate);

    let selected = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("dead-driver settle must not stall preemption")
        .expect("preemption task")
        .expect("pure mid-turn reuses the settled session");
    assert!(Arc::ptr_eq(&selected.session, &session));
    assert!(
        selected.session.pending_tools.lock().await.is_empty(),
        "settled cancel abandons local pending-tool ownership for the follow-up"
    );
    drop(selected);
    app.shutdown().await;
}

#[test]
fn reports_all_preemption_cancellation_outcomes() {
    for cancellation in [
        Ok(TurnCancellation::Settled),
        Ok(TurnCancellation::Unsupported),
        Err(anyhow::anyhow!("provider failure")),
    ] {
        report_cancellation(cancellation, "thread");
    }
}

fn start_preemption(
    app: Arc<AgentBackend>,
    session: Arc<Session>,
    request: MessagesRequest,
) -> tokio::task::JoinHandle<Option<SelectedSession>> {
    let messages = request.messages.clone();
    tokio::spawn(run_preemption(app, session, request, messages))
}

async fn run_preemption(
    app: Arc<AgentBackend>,
    session: Arc<Session>,
    request: MessagesRequest,
    messages: Vec<Value>,
) -> Option<SelectedSession> {
    select_matching_session(
        vec![session],
        &request,
        &Arc::from("signature"),
        &messages,
        &app,
    )
    .await
}

async fn test_app() -> Arc<AgentBackend> {
    let root = tempfile::tempdir().expect("app-server fixture");
    let source = root.path().join("source");
    std::fs::create_dir(&source).expect("source home");
    std::fs::write(source.join("auth.json"), "{}").expect("auth file");
    let program = root.path().join("codex");
    std::fs::write(
            &program,
            "#!/bin/sh\nread line\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nwhile read line; do :; done\n",
        )
        .expect("mock program");
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
        .expect("mock program permissions");
    let server = AppServer::spawn_with_program("main-model", &program, &source, root.path())
        .await
        .expect("mock app server");
    AgentBackend::codex(server)
}

fn stopped_acp_app() -> Arc<AgentBackend> {
    AgentBackend::grok(crate::grok_acp::GrokAcp::stopped_for_test())
}

fn request(model: &str) -> MessagesRequest {
    MessagesRequest {
        model: model.to_owned(),
        system: json!("system"),
        messages: vec![
            json!({"role":"user","content":"first"}),
            json!({"role":"user","content":"follow-up"}),
        ],
        tools: Vec::new(),
        stream: true,
        output_config: json!({}),
        metadata: json!({"user_id":"client"}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

fn subagent_request() -> MessagesRequest {
    let mut request = request("main-model");
    request.metadata = json!({
        "user_id":"client",
        "_claudex_transport_identity":{
            "session_id":"s1",
            "agent_id":"agent-1",
            "parent_agent_id":"parent-1"
        }
    });
    request
}

fn session(model: &str, user_id: Option<&str>) -> Arc<Session> {
    session_named(model, user_id, "signature")
}

fn session_named(model: &str, user_id: Option<&str>, signature: &str) -> Arc<Session> {
    let slots = Arc::new(Semaphore::new(1));
    Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: model.to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from(signature),
        transcript: Mutex::new(vec![json!({"role":"user","content":"first"})]),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(Default::default()),
        external_tool_names: HashMap::new(),
        client_user_id: user_id.map(str::to_owned),
        claude_session_id: None,
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: slots.try_acquire_owned().expect("session slot"),
    })
}
