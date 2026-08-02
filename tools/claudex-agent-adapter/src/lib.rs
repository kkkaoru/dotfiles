#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, allow(unused_features))]

pub mod agent_backend;
pub mod anthropic;
pub mod app_server;
pub mod build_support;
pub mod copilot_acp;
pub mod coverage_gate;
pub mod grok_acp;
mod http_api;
pub mod launcher;
pub(crate) mod logging;
pub mod parallel_scheduler;
pub mod path_env;
pub mod provider_config;
pub mod runtime;
mod subagent_policy;
mod web_search;
mod working_directory;

pub use http_api::http_router;

pub const ADAPTER_PROTOCOL_VERSION: u64 = 24;
pub(crate) const NONINTERACTIVE_CHILD_ENV: &str = "CLAUDEX_NONINTERACTIVE_CHILD";
/// Legacy gateway aliases remain accepted on requests from older Claude Code sessions.
pub(crate) const DISCOVERY_MODEL_PREFIX: &str = "claude-claudex-";
