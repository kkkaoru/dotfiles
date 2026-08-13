use std::{collections::HashMap, sync::Arc, time::Instant};

use serde_json::json;
use tokio::sync::{Mutex, Semaphore};

use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};
use crate::anthropic::{Bridge, Session};
use crate::provider_config;

fn session_with_id(id: &str) -> Arc<Session> {
    Arc::new(Session {
        thread_id: "thread".to_owned(),
        model: "main".to_owned(),
        disabled_subagent_models: Default::default(),
        signature: Arc::from("sig"),
        transcript: Mutex::new(Vec::new()),
        pending_tools: Mutex::new(HashMap::new()),
        consumed_tool_ids: Mutex::new(Default::default()),
        external_tool_names: HashMap::new(),
        client_user_id: None,
        claude_session_id: Some(id.to_owned()),
        gate: Arc::new(Mutex::new(())),
        last_activity: std::sync::Mutex::new(Instant::now()),
        pending_since: std::sync::Mutex::new(None),
        turn_progress: Default::default(),
        adopted_thread_id: Default::default(),
        _slot: Arc::new(Semaphore::new(1))
            .try_acquire_owned()
            .expect("session slot"),
    })
}

#[test]
fn routed_models_advertise_codex_terra_for_main_selection() {
    let catalog = provider_config::load(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.config/claudex/providers.json")
            .as_path(),
    )
    .expect("load repository providers.json")
    .model_catalog;
    let bridge = Bridge::new_with_backend(
        AgentBackend::spawn_routes(&[BackendRoute::new(
            "gpt-5.6-luna",
            BackendKind::CodexAppServer,
        )]),
        "gpt-5.6-luna".to_owned(),
    )
    .with_model_catalog(catalog);
    let models = bridge.routed_models();
    assert!(
        models.iter().any(|model| model == "gpt-5.6-luna"),
        "default Codex model must stay listed: {models:?}"
    );
    assert!(
        models.iter().any(|model| model == "gpt-5.6-terra"),
        "gpt-5.6-terra must appear on GET /v1/models: {models:?}"
    );
}

#[tokio::test]
async fn busy_claude_session_ids_skip_idle_tui_sessions() {
    let bridge = Bridge::new_with_backend(
        AgentBackend::spawn_routes(&[BackendRoute::new(
            "gpt-5.6-luna",
            BackendKind::CodexAppServer,
        )]),
        "gpt-5.6-luna".to_owned(),
    );
    let idle = session_with_id("idle-tui");
    let busy_gate = session_with_id("busy-gate");
    let _guard = busy_gate.gate.clone().try_lock_owned().expect("lock gate");
    let busy_pending = session_with_id("busy-pending");
    busy_pending
        .pending_tools
        .lock()
        .await
        .insert("tool-1".to_owned(), json!({"type": "tool_use"}));
    let detached = session_with_id("detached");
    bridge.sessions.lock().await.extend([
        Arc::clone(&idle),
        Arc::clone(&busy_gate),
        Arc::clone(&busy_pending),
    ]);
    bridge.detached_sessions.lock().await.push(detached);
    assert_eq!(
        bridge.busy_claude_session_ids().await,
        vec![
            "busy-gate".to_owned(),
            "busy-pending".to_owned(),
            "detached".to_owned()
        ]
    );
    assert_eq!(
        bridge.active_claude_session_ids().await,
        vec![
            "busy-gate".to_owned(),
            "busy-pending".to_owned(),
            "detached".to_owned(),
            "idle-tui".to_owned()
        ]
    );
}

#[tokio::test]
async fn session_id_helpers_skip_blank_claude_session_ids() {
    let bridge = Bridge::new_with_backend(
        AgentBackend::spawn_routes(&[BackendRoute::new(
            "gpt-5.6-luna",
            BackendKind::CodexAppServer,
        )]),
        "gpt-5.6-luna".to_owned(),
    );
    let blank = session_with_id("");
    let blank_busy = session_with_id("");
    let named = session_with_id("named");
    let _blank_busy_guard = blank_busy
        .gate
        .clone()
        .try_lock_owned()
        .expect("lock blank busy gate");
    let _guard = named.gate.clone().try_lock_owned().expect("lock gate");
    bridge
        .sessions
        .lock()
        .await
        .extend([blank, blank_busy, Arc::clone(&named)]);
    assert_eq!(
        bridge.active_claude_session_ids().await,
        vec!["named".to_owned()]
    );
    assert_eq!(
        bridge.busy_claude_session_ids().await,
        vec!["named".to_owned()],
        "busy sessions with blank claude ids must stay out of the busy id list"
    );
}

#[test]
fn routed_models_skip_workers_with_blank_model_fields() {
    let mut catalog = provider_config::ModelCatalog::default();
    catalog.push_worker_unchecked_for_tests(provider_config::WorkerRoute::new(
        "claudex-blank",
        "",
        "high",
    ));
    catalog.push_worker_unchecked_for_tests(provider_config::WorkerRoute::new(
        "claudex-keep",
        "kept-model",
        "high",
    ));
    let bridge = Bridge::new_with_backend(
        AgentBackend::spawn_routes(&[BackendRoute::new("kept-model", BackendKind::CodexAppServer)]),
        "fallback-model".to_owned(),
    )
    .with_model_catalog(catalog);
    let models = bridge.routed_models();
    assert!(models.contains(&"kept-model".to_owned()));
    assert!(!models.iter().any(String::is_empty));
}

#[test]
fn provider_session_scopes_report_one_pool_per_claude_tui_session() {
    let bridge = Bridge::new_with_backend(
        AgentBackend::spawn_routes(&[BackendRoute::new(
            "gpt-5.6-luna",
            BackendKind::CodexAppServer,
        )]),
        "gpt-5.6-luna".to_owned(),
    );
    assert_eq!(bridge.provider_session_scope_count(), 0);
    let first = bridge.app_for(Some("tui-a"));
    let second = bridge.app_for(Some("tui-b"));
    let reused = bridge.app_for(Some("tui-a"));
    assert!(!std::sync::Arc::ptr_eq(&first, &second));
    assert!(std::sync::Arc::ptr_eq(&first, &reused));
    assert_eq!(bridge.provider_session_scope_count(), 2);
    assert_eq!(
        bridge
            .provider_session_scopes()
            .into_iter()
            .map(|scope| scope.claude_session_id)
            .collect::<Vec<_>>(),
        vec!["tui-a".to_owned(), "tui-b".to_owned()]
    );
}
