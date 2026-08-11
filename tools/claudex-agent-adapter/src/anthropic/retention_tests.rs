use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::json;
use tokio::sync::{Mutex, Semaphore};

use super::*;
use crate::{
    agent_backend::{AgentBackend, BackendKind, BackendRoute},
    app_server::AppServer,
};
use super::super::Session;

#[test]
fn retains_idle_provider_context_for_follow_up_window() {
    assert_eq!(IDLE_SESSION_TTL, Duration::from_secs(120 * 60));
    assert!(IDLE_SESSION_TTL > PENDING_SESSION_TTL);
}

#[tokio::test]
async fn bridge_evicts_expired_tools_and_sends_cancellation() {
    let root = tempfile::tempdir().unwrap();
    let app = spawn_app(root.path(), true).await;
    let bridge = Bridge::new(app, "model".to_owned());
    let expired = Instant::now() - PENDING_SESSION_TTL;
    bridge.sessions.lock().await.push(session(expired));

    bridge.evict_oldest_idle_session().await;

    assert!(bridge.sessions.lock().await.is_empty());
    bridge.evict_oldest_idle_session().await;
}

#[tokio::test]
async fn bridge_sweeps_an_expired_idle_session_during_request_maintenance() {
    let root = tempfile::tempdir().unwrap();
    let app = spawn_app(root.path(), true).await;
    let bridge = Bridge::new(app, "model".to_owned());
    let sweep_at = *bridge.next_session_sweep.lock().unwrap();
    let idle = session(sweep_at - IDLE_SESSION_TTL);
    idle.pending_tools.lock().await.clear();
    *idle.pending_since.lock().unwrap() = None;
    bridge.sessions.lock().await.push(idle);

    assert_eq!(
        bridge
            .sweep_idle_sessions_if_due_at(sweep_at - Duration::from_nanos(1))
            .await,
        0
    );
    assert_eq!(bridge.sweep_idle_sessions_if_due_at(sweep_at).await, 1);
    assert_eq!(bridge.sweep_idle_sessions_if_due_at(sweep_at).await, 0);
    let next_sweep = *bridge.next_session_sweep.lock().unwrap();
    assert_eq!(bridge.sweep_idle_sessions_if_due_at(next_sweep).await, 0);

    assert!(bridge.sessions.lock().await.is_empty());
}

#[tokio::test]
async fn eviction_tolerates_a_closed_app_server() {
    let root = tempfile::tempdir().unwrap();
    let app = spawn_app(root.path(), false).await;
    let bridge = Bridge::new(app, "model".to_owned());
    let expired = Instant::now() - PENDING_SESSION_TTL;
    bridge.sessions.lock().await.push(session(expired));
    wait_until_stopped(&bridge).await;

    bridge.evict_oldest_idle_session().await;

    assert!(bridge.sessions.lock().await.is_empty());
}

#[tokio::test]
async fn retains_a_newer_candidate_seen_after_the_oldest() {
    let now = Instant::now();
    let oldest_activity = now - IDLE_SESSION_TTL - Duration::from_secs(2);
    let oldest = session(oldest_activity);
    oldest.pending_tools.lock().await.clear();
    let newer = session(now - IDLE_SESSION_TTL - Duration::from_secs(1));
    newer.pending_tools.lock().await.clear();
    let sessions = Mutex::new(vec![Arc::clone(&oldest), newer]);
    drop(oldest);

    let evicted = take_oldest_evictable_at(&sessions, now).await.unwrap();

    assert_eq!(*evicted.last_activity.lock().unwrap(), oldest_activity);
}

#[tokio::test]
async fn bridge_sweep_releases_session_scoped_provider_pool() {
    let bridge = Bridge::new_with_backend(
        AgentBackend::spawn_routes(&[BackendRoute::new(
            "main",
            BackendKind::CodexAppServer,
        )]),
        "main".to_owned(),
    );
    let _ = bridge.app_for(Some("ttl-scope"));
    let AgentBackend::SessionScoped(scopes) = bridge.app.as_ref() else {
        panic!("spawn_routes must build SessionScoped");
    };
    assert_eq!(scopes.scope_count(), 1);

    let sweep_at = *bridge.next_session_sweep.lock().unwrap();
    let idle = session_with_claude_id(sweep_at - IDLE_SESSION_TTL, "ttl-scope");
    idle.pending_tools.lock().await.clear();
    *idle.pending_since.lock().unwrap() = None;
    bridge.sessions.lock().await.push(idle);

    assert_eq!(bridge.sweep_idle_sessions_if_due_at(sweep_at).await, 1);
    let AgentBackend::SessionScoped(scopes) = bridge.app.as_ref() else {
        panic!("spawn_routes must build SessionScoped");
    };
    assert_eq!(
        scopes.scope_count(),
        0,
        "idle TTL must shut down the Claude session provider pool"
    );
}

#[tokio::test]
async fn release_keeps_scope_while_a_detached_session_still_references_it() {
    let bridge = Bridge::new_with_backend(
        AgentBackend::spawn_routes(&[BackendRoute::new(
            "main",
            BackendKind::CodexAppServer,
        )]),
        "main".to_owned(),
    );
    let _ = bridge.app_for(Some("kept-scope"));
    let detached = session_with_claude_id(Instant::now(), "kept-scope");
    detached.pending_tools.lock().await.clear();
    *detached.pending_since.lock().unwrap() = None;
    bridge.detached_sessions.lock().await.push(detached);

    bridge
        .release_provider_scope_if_unused(Some("kept-scope"))
        .await;
    let AgentBackend::SessionScoped(scopes) = bridge.app.as_ref() else {
        panic!("spawn_routes must build SessionScoped");
    };
    assert_eq!(
        scopes.scope_count(),
        1,
        "detached Claude sessions must keep their provider pool"
    );
    assert!(Arc::ptr_eq(
        &bridge.app_for(Some("kept-scope")),
        &bridge.app_for_session(
            bridge
                .detached_sessions
                .lock()
                .await
                .first()
                .expect("detached session")
        )
    ));
}

fn session(activity: Instant) -> Arc<Session> {
    session_with_claude_id(activity, "")
}

fn session_with_claude_id(activity: Instant, claude_session_id: &str) -> Arc<Session> {
    let slots = Arc::new(Semaphore::new(1));
    Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "main-model".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("signature"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::from([("tool".to_owned(), json!(9))])),
        consumed_tool_ids: Mutex::new(Default::default()),
        external_tool_names: HashMap::new(),
        client_user_id: None,
        claude_session_id: (!claude_session_id.is_empty()).then(|| claude_session_id.to_owned()),
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(activity),
        pending_since: std::sync::Mutex::new(Some(activity)),
        _slot: slots.try_acquire_owned().unwrap(),
    })
}

async fn spawn_app(root: &Path, keep_open: bool) -> Arc<AppServer> {
    let source = root.join("source");
    let isolated = root.join("isolated");
    let program = root.join("app-server");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("auth.json"), "{}").unwrap();
    let tail = mock_app_server_tail(keep_open);
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\nread line\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\nread line\n{tail}\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    AppServer::spawn_with_program("model", program, &source, &isolated)
        .await
        .unwrap()
}

fn mock_app_server_tail(keep_open: bool) -> &'static str {
    ["exit 0", "while read line; do :; done"][usize::from(keep_open)]
}

async fn wait_until_stopped(bridge: &Bridge) {
    tokio::time::timeout(Duration::from_secs(1), wait_while_alive(bridge))
        .await
        .expect("fixture app-server closes");
}

async fn wait_while_alive(bridge: &Bridge) {
    while bridge.app.is_alive() {
        tokio::task::yield_now().await;
    }
}
