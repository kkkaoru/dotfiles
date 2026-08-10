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
