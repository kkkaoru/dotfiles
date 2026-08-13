#[cfg(test)]
use std::path::PathBuf;
use std::{ffi::OsString, net::SocketAddr, time::Duration};

use anyhow::Result;

mod claude_process;
mod claude_relay;
#[cfg(test)]
use claude_relay::reject_model_override;
#[cfg(test)]
use claude_relay::relay_filtered;
#[allow(unused_imports)]
use claude_relay::requires_authentication;
mod cli_swap;
mod daemon_arguments;
#[allow(unused_imports)]
use daemon_arguments::{
    route_descriptions, search_worker_route_descriptions, worker_route_descriptions,
};
mod daemon_process;
mod daemon_start;
mod ensure;
mod fallback;
mod handover;
mod health;
mod process_io;
#[cfg(test)]
use process_io::exit_code;
mod live;
mod promote;
pub(crate) use daemon_process::terminate_retained_serve;
pub(crate) use live::{
    RETAINED_STATE_ENV, RetainedGeneration, SERVICE_LISTEN_ENV, clear_retained,
    forget_retained_session, load_retained_from_env, read_retained,
};
mod installed_adapter;
mod launcher_lock;
mod launcher_logs;
mod macos_notify;
mod macos_notify_dispatch;
mod pending_hot_swap;
mod preflight;
mod program_identity;
mod recovery;
mod recovery_manifest;
mod resume;
mod session_process;
#[allow(unused_imports)]
use crate::ADAPTER_PROTOCOL_VERSION;
use crate::agent_backend::BackendRoute;
use claude_process::ClaudeProcess;
#[cfg(test)]
use daemon_arguments::{daemon_arguments, hot_swap_wait_arguments};
#[cfg(test)]
use daemon_start::start_adapter;
#[cfg(test)]
use health::Health;
use health::{authenticates, fetch_health, wait_until_ready, wait_until_recovery_ready};

pub(super) const LOCAL_TOKEN: &str = "claudex-local";
#[cfg(not(test))]
const START_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(all(test, coverage_nightly))]
const START_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(all(test, not(coverage_nightly)))]
const START_TIMEOUT: Duration = Duration::from_secs(2);
const START_INITIAL_POLL_DELAY: Duration = Duration::from_millis(10);
const START_MAX_POLL_DELAY: Duration = Duration::from_millis(100);
pub(crate) const SERVICE_CONFIG_FINGERPRINT_ENV: &str = "CLAUDEX_SERVICE_CONFIG_FINGERPRINT";
pub(crate) const RECOVERY_MANIFEST_ENV: &str = "CLAUDEX_RECOVERY_MANIFEST";

#[derive(Clone, Debug)]
pub struct AdapterOptions {
    pub routes: Vec<BackendRoute>,
    pub model: String,
    pub listen: SocketAddr,
    pub subscription_max_processes: usize,
    pub subscription_timeout_minutes: u64,
    pub subagent_hard_timeout_seconds: Option<std::num::NonZeroU64>,
    pub model_catalog: crate::provider_config::ModelCatalog,
}

mod service_config;
pub(crate) use service_config::ServiceConfig;

pub(crate) fn recovery_generation() -> Option<String> {
    recovery_manifest::generation_from_environment()
}

pub async fn ensure_running(options: AdapterOptions) -> Result<String> {
    let config = ServiceConfig::new(options)?;
    ensure::run(&config, ensure::Mode::Ensure).await
}

pub use cli_swap::{ensure_running_cli, hot_swap_cli};

/// Run macOS notify dedupe/delivery in the on-disk install binary.
pub fn run_internal_notify(arguments: Vec<OsString>) -> Result<()> {
    macos_notify_dispatch::run_internal(arguments)
}

/// Opt interactive CLI `ensure` / `hot-swap` into macOS swap banners when unset.
pub fn opt_in_cli_swap_notify() {
    macos_notify_dispatch::opt_in_cli_swap_notify()
}

/// Update the listener without ending Claude Code / claudex sessions.
/// Handover-capable daemons warm-start the new build, then cut `:port` over so
/// idle TUI turns use the new generation immediately. Only in-flight sessions
/// stay sticky on the retained generation. Legacy busy daemons still get a
/// current-build fallback plus an idle waiter, and `live.<port>.json` always
/// points at the generation new sessions should use.
pub async fn hot_swap(options: AdapterOptions, wait_idle: bool) -> Result<String> {
    let config = ServiceConfig::new(options)?;
    ensure::run(
        &config,
        if wait_idle {
            ensure::Mode::WaitIdle
        } else {
            ensure::Mode::HotSwap
        },
    )
    .await
}

#[path = "claude_launch.rs"]
mod claude_launch;
pub use claude_launch::run_claude;

#[cfg(test)]
use health::wait_until_ready_with;

#[cfg(test)]
#[cfg(unix)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "../tests/support/wait_published.rs"]
pub(crate) mod wait_published;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "launcher_tests.rs"]
mod tests;
