use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Instant;

use axum::{
    Json, Router,
    http::StatusCode,
    routing::get,
};
use serde_json::json;

use crate::{ADAPTER_PROTOCOL_VERSION, anthropic::Bridge, listen_handover::ListenHandover};

#[derive(Clone)]
pub(super) struct HealthRouteState {
    pub(super) bridge: Arc<Bridge>,
    pub(super) model: String,
    pub(super) active_http_requests: Arc<AtomicUsize>,
    pub(super) active_provider_turns: Arc<AtomicUsize>,
    pub(super) last_work_at: Arc<Mutex<Instant>>,
    pub(super) subscription_max_processes: usize,
    pub(super) subscription_timeout_minutes: u64,
    pub(super) backend_routes: Vec<String>,
    pub(super) worker_routes: Vec<String>,
    pub(super) search_worker_routes: Vec<String>,
    pub(super) subagent_hard_timeout_seconds: Option<u64>,
    pub(super) handover: Option<ListenHandover>,
}

pub(super) fn mount_health_route(router: Router, state: HealthRouteState) -> Router {
    router.route(
        "/health",
        get(move || {
            let state = state.clone();
            async move { health_response(state).await }
        }),
    )
}

fn idle_seconds(last_work_at: &Mutex<Instant>, busy: bool) -> u64 {
    if busy {
        // Defense in depth if a busy sample races ahead of ActiveCounter touch.
        touch_last_work(last_work_at);
        return 0;
    }
    let last = last_work_at
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    last.elapsed().as_secs()
}

pub(super) fn touch_last_work(last_work_at: &Mutex<Instant>) {
    let mut last = last_work_at
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *last = Instant::now();
}

async fn health_response(state: HealthRouteState) -> (StatusCode, Json<serde_json::Value>) {
    let status = if state.bridge.is_alive() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let session_ids = state.bridge.active_claude_session_ids().await;
    let busy_session_ids = state.bridge.busy_claude_session_ids().await;
    let listen = state
        .handover
        .as_ref()
        .map(|handover| handover.advertised_addr().to_string());
    let active_subagent_models = state.bridge.active_subagent_models();
    let http = state.active_http_requests.load(Ordering::Relaxed);
    let turns = state.active_provider_turns.load(Ordering::Relaxed);
    let busy = http > 0
        || turns > 0
        || active_subagent_models.values().copied().sum::<usize>() > 0;
    let idle_seconds = idle_seconds(&state.last_work_at, busy);
    (
        status,
        Json(json!({
            "status":if status.is_success() { "ok" } else { "unavailable" },
            "pid":std::process::id(),
            "protocol_version":ADAPTER_PROTOCOL_VERSION,
            "build_id":env!("CLAUDEX_BUILD_ID"),
            "codex_config_fingerprint":std::env::var(crate::app_server::CODEX_CONFIG_FINGERPRINT_ENV).unwrap_or_default(),
            "service_config_fingerprint":std::env::var(crate::launcher::SERVICE_CONFIG_FINGERPRINT_ENV).unwrap_or_default(),
            "backend_routes":state.backend_routes,
            "worker_routes":state.worker_routes,
            "search_worker_routes":state.search_worker_routes,
            "started_models":state.bridge.started_models(),
            "model_concurrency":state.bridge.model_concurrency(),
            "active_subagent_models":active_subagent_models,
            "active_subagent_agent_ids":state.bridge.active_subagent_agent_ids(),
            "recent_subagent_agent_ids":state.bridge.recent_subagent_agent_ids(),
            "model":state.model,
            "session_capacity":state.bridge.session_capacity(),
            "session_slots_used":state.bridge.used_session_slots(),
            "active_provider_turns":turns,
            "active_http_requests":http,
            "idle_seconds":idle_seconds,
            "subscription_max_processes":state.subscription_max_processes,
            "subscription_timeout_minutes":state.subscription_timeout_minutes,
            "subagent_hard_timeout_seconds":state.subagent_hard_timeout_seconds,
            "recovery_generation":crate::launcher::recovery_generation(),
            "listener_handover":state.handover.is_some(),
            "listen":listen,
            "active_claude_session_ids":session_ids,
            "busy_claude_session_ids":busy_session_ids
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn touch_last_work_resets_idle_seconds_without_busy_health_sample() {
        let last_work_at = Mutex::new(Instant::now() - Duration::from_secs(120));
        assert!(idle_seconds(&last_work_at, false) >= 120);
        touch_last_work(&last_work_at);
        assert!(
            idle_seconds(&last_work_at, false) <= 1,
            "real work touch must start the idle clock near zero"
        );
    }

    #[test]
    fn busy_health_sample_still_reports_zero_idle() {
        let last_work_at = Mutex::new(Instant::now() - Duration::from_secs(120));
        assert_eq!(idle_seconds(&last_work_at, true), 0);
        assert!(idle_seconds(&last_work_at, false) <= 1);
    }
}

