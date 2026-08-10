use super::*;
use crate::ADAPTER_PROTOCOL_VERSION;
use crate::agent_backend::{BackendKind, BackendRoute};
use crate::launcher::health::Health;
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::super::{AdapterOptions, LOCAL_TOKEN, ServiceConfig};

fn health(listener_handover: bool, pid: Option<u32>) -> Health {
    Health {
        status: "ok".to_owned(),
        pid,
        protocol_version: ADAPTER_PROTOCOL_VERSION,
        build_id: "old-build".to_owned(),
        model: "opus".to_owned(),
        codex_config_fingerprint: "codex".to_owned(),
        service_config_fingerprint: "service".to_owned(),
        backend_routes: Vec::new(),
        worker_routes: Vec::new(),
        search_worker_routes: Vec::new(),
        subscription_max_processes: 20,
        subscription_timeout_minutes: 120,
        subagent_hard_timeout_seconds: None,
        recovery_generation: None,
        active_http_requests: 1,
        active_provider_turns: 1,
        active_subagent_models: BTreeMap::new(),
        listener_handover,
        listen: Some("127.0.0.1:8318".to_owned()),
        active_claude_session_ids: vec!["session-a".to_owned()],
        busy_claude_session_ids: Vec::new(),
    }
}

#[test]
fn handover_requires_a_capable_daemon_pid() {
    assert!(!handover_supported(&health(false, Some(12))));
    assert!(!handover_supported(&health(true, None)));
    assert!(!handover_supported(&health(true, Some(0))));
    assert!(handover_supported(&health(true, Some(12))));
}

#[tokio::test]
async fn try_canonical_skips_legacy_daemons_without_handover() {
    let config = ServiceConfig {
        options: AdapterOptions {
            routes: vec![BackendRoute::new("test-model", BackendKind::CodexAppServer)],
            listen: "127.0.0.1:8318".parse().unwrap(),
            model: "test-model".to_owned(),
            subscription_max_processes: 20,
            subscription_timeout_minutes: 120,
            subagent_hard_timeout_seconds: None,
            model_catalog: crate::provider_config::ModelCatalog::default(),
        },
        token: LOCAL_TOKEN.to_owned(),
        codex_config_fingerprint: "codex".to_owned(),
        service_config_fingerprint: "service".to_owned(),
        executable: PathBuf::from("/tmp/claudex-agent-adapter"),
        log_path: PathBuf::from("/tmp/claudex/adapter.log"),
        lock_path: PathBuf::from("/tmp/claudex/adapter.lock"),
    };
    let url = try_canonical(&reqwest::Client::new(), &config, &health(false, Some(9)))
        .await
        .expect("legacy skip");
    assert_eq!(url, None);
}

#[test]
fn retains_only_busy_sessions_when_the_new_health_field_is_present() {
    let mut health = health(true, Some(12));
    health.busy_claude_session_ids = vec!["busy-a".to_owned()];
    health.active_claude_session_ids = vec!["busy-a".to_owned(), "idle-tui".to_owned()];
    assert_eq!(retained_session_ids(&health), ["busy-a"]);
}

#[test]
fn retains_all_active_sessions_on_legacy_busy_health() {
    let mut health = health(true, Some(12));
    health.active_http_requests = 1;
    health.busy_claude_session_ids.clear();
    assert_eq!(retained_session_ids(&health), ["session-a"]);
}

#[test]
fn retains_no_sessions_for_an_idle_tui() {
    let mut health = health(true, Some(12));
    health.active_http_requests = 0;
    health.active_provider_turns = 0;
    health.busy_claude_session_ids.clear();
    health.active_claude_session_ids = vec!["idle-tui".to_owned()];
    assert!(retained_session_ids(&health).is_empty());
}
