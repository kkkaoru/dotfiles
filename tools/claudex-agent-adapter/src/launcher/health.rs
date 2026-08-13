use std::{collections::BTreeMap, time::Duration};

use serde::Deserialize;

use super::{
    ServiceConfig, route_descriptions, search_worker_route_descriptions, worker_route_descriptions,
};
use crate::ADAPTER_PROTOCOL_VERSION;

#[derive(Debug, Deserialize)]
pub(super) struct Health {
    pub(super) status: String,
    pub(super) pid: Option<u32>,
    pub(super) protocol_version: u64,
    #[serde(rename = "build_id")]
    pub(super) build_id: String,
    #[serde(default)]
    pub(super) model: String,
    #[serde(default)]
    pub(super) codex_config_fingerprint: String,
    #[serde(default)]
    pub(super) service_config_fingerprint: String,
    #[serde(default)]
    pub(super) backend_routes: Vec<String>,
    #[serde(default)]
    pub(super) worker_routes: Vec<String>,
    #[serde(default)]
    pub(super) search_worker_routes: Vec<String>,
    pub(super) subscription_max_processes: usize,
    pub(super) subscription_timeout_minutes: u64,
    #[serde(default)]
    pub(super) subagent_hard_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub(super) recovery_generation: Option<String>,
    /// Number of HTTP requests currently served by the adapter.
    /// Older adapters omit this field, so deserialization remains compatible.
    #[serde(default)]
    pub(super) active_http_requests: usize,
    /// Number of provider turns currently in flight.
    /// Older adapters omit this field, so deserialization remains compatible.
    #[serde(default)]
    pub(super) active_provider_turns: usize,
    /// Native Claude Code SubAgents still running after the parent turn ended.
    /// Older adapters omit this field, so deserialization remains compatible.
    #[serde(default)]
    pub(super) active_subagent_models: BTreeMap<String, usize>,
    /// Current binary can rebind its listen socket without exiting.
    /// Older adapters omit this field.
    #[serde(default)]
    pub(super) listener_handover: bool,
    /// Advertised listen address after an in-process rebind.
    #[serde(default)]
    pub(super) listen: Option<String>,
    /// Claude Code session ids currently attached to this generation.
    #[serde(default)]
    pub(super) active_claude_session_ids: Vec<String>,
    /// Claude Code session ids with in-flight turns or pending tools.
    /// Older adapters omit this field; when absent, busy work falls back to
    /// `active_claude_session_ids` only while `has_active_work()` is true.
    #[serde(default)]
    pub(super) busy_claude_session_ids: Vec<String>,
    /// Seconds since this daemon last observed live work. Older adapters omit
    /// the field; sticky / release_idle then treat quiet health as immediately idle.
    #[serde(default)]
    pub(super) idle_seconds: Option<u64>,
    /// In-flight SubAgent agentIds. Older adapters omit this field.
    #[serde(default)]
    pub(super) active_subagent_agent_ids: Vec<String>,
    /// Warm SubAgent agentIds → seconds since last observation. Older adapters omit.
    #[serde(default)]
    pub(super) recent_subagent_agent_ids: BTreeMap<String, u64>,
}

impl Health {
    pub(super) fn active_subagent_count(&self) -> usize {
        self.active_subagent_models.values().copied().sum()
    }

    pub(super) fn has_active_work(&self) -> bool {
        self.active_http_requests > 0
            || self.active_provider_turns > 0
            || self.active_subagent_count() > 0
    }

    pub(super) fn within_sticky_idle_grace(&self) -> bool {
        crate::sticky_grace::within_sticky_idle_grace_secs(self.idle_seconds)
    }
}

impl ServiceConfig {
    pub(super) fn matches(&self, health: &Health) -> bool {
        // Protocol/config compatibility is separate from build freshness. The handover
        // state machine checks the build ID without interrupting accepted responses.
        health.status == "ok"
            && health.protocol_version == ADAPTER_PROTOCOL_VERSION
            && health.model == self.options.model
            && health.codex_config_fingerprint == self.codex_config_fingerprint
            && health.service_config_fingerprint == self.service_config_fingerprint
            && health.backend_routes == route_descriptions(&self.options.routes)
            && health.worker_routes == worker_route_descriptions(&self.options.model_catalog)
            && health.search_worker_routes
                == search_worker_route_descriptions(&self.options.model_catalog)
            && health.subscription_max_processes == self.options.subscription_max_processes
            && health.subscription_timeout_minutes == self.options.subscription_timeout_minutes
            && health.subagent_hard_timeout_seconds
                == self
                    .options
                    .subagent_hard_timeout_seconds
                    .map(std::num::NonZeroU64::get)
    }
}

pub(super) async fn fetch_health(
    client: &reqwest::Client,
    config: &ServiceConfig,
) -> Option<Health> {
    client
        .get(format!("{}/health", config.base_url()))
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()
}

pub(super) async fn authenticates(client: &reqwest::Client, config: &ServiceConfig) -> bool {
    let timeout = Duration::from_millis(500);
    client
        .get(format!("{}/v1/models", config.base_url()))
        .bearer_auth(&config.token)
        .timeout(timeout)
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

#[path = "health_wait.rs"]
mod wait;
#[cfg(test)]
pub(super) use wait::wait_until_ready_with;
pub(super) use wait::{wait_until_ready, wait_until_recovery_ready};
