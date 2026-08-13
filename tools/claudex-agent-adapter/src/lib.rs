#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, allow(unused_features))]

pub mod agent_backend;
pub mod anthropic;
pub mod app_server;
pub mod build_support;
mod cache_hygiene;
pub mod command_code_acp;
pub mod copilot_acp;
pub mod coverage_gate;
pub mod grok_acp;
mod http_api;
mod launch_mcp;
pub mod launcher;
pub(crate) mod listen_handover;
pub(crate) mod logging;
mod model_id;
pub mod parallel_scheduler;
pub mod path_env;
pub mod provider_config;
pub mod runtime;
pub(crate) mod sticky_grace;
mod subagent_policy;
mod web_search;
mod working_directory;

pub use http_api::http_router;

pub const ADAPTER_PROTOCOL_VERSION: u64 = 24;
pub(crate) const NONINTERACTIVE_CHILD_ENV: &str = "CLAUDEX_NONINTERACTIVE_CHILD";
/// Claude Code gateway discovery only keeps model ids that match `^(claude|anthropic)`.
/// Prefix every non-Anthropic provider model so `/model` can list and select it; request
/// routing strips this prefix before matching configured backends.
pub(crate) const DISCOVERY_MODEL_PREFIX: &str = "claude-claudex-";

/// Id advertised on `GET /v1/models` for Claude Code gateway discovery.
pub(crate) fn discovery_model_id(model: &str) -> String {
    if model.starts_with("claude") || model.starts_with("anthropic") {
        model.to_owned()
    } else {
        format!("{DISCOVERY_MODEL_PREFIX}{model}")
    }
}
