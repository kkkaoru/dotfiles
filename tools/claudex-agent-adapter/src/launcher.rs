#[cfg(test)]
use std::path::PathBuf;
use std::{
    ffi::OsString,
    net::SocketAddr,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use uuid::Uuid;

mod claude_process;
mod claude_relay;
#[cfg(test)]
use claude_relay::relay_filtered;
#[allow(unused_imports)]
use claude_relay::requires_authentication;
use claude_relay::{reject_model_override, relay_stderr};
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
use crate::{agent_backend::BackendRoute, subagent_policy as policy, working_directory};
use claude_process::ClaudeProcess;
#[cfg(test)]
use daemon_arguments::{daemon_arguments, hot_swap_wait_arguments};
#[cfg(test)]
use daemon_start::start_adapter;
#[cfg(test)]
use health::Health;
use health::{authenticates, fetch_health, wait_until_ready, wait_until_recovery_ready};
use resume::{prepare_arguments, session_id_for_launch};

pub(super) const LOCAL_TOKEN: &str = "claudex-local";
#[cfg(not(test))]
const START_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(all(test, coverage_nightly))]
const START_TIMEOUT: Duration = Duration::from_secs(45);
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

fn acquire_resume_session_lock(
    config: &ServiceConfig,
    arguments: &[OsString],
) -> Result<Option<launcher_lock::LauncherLock>> {
    let Some(resume_id) = resume::session_lock_id(arguments) else {
        return Ok(None);
    };
    if session_process::another_resume_launcher_is_active(&resume_id)? {
        bail!("{}", resume_session_busy_message(&resume_id));
    }
    let cache = config
        .log_path
        .parent()
        .context("adapter log has no parent directory")?;
    let path = launcher_logs::session_lock_path(cache, &resume_id);
    let lock = launcher_lock::try_acquire(&path)?
        .ok_or_else(|| anyhow::anyhow!("{}", resume_session_busy_message(&resume_id)))?;
    Ok(Some(lock))
}

fn resume_session_busy_message(resume_id: &str) -> String {
    format!(
        "resume session '{resume_id}' is already active; continue in the existing Claude Code process or use --fork-session"
    )
}

pub async fn run_claude(
    options: AdapterOptions,
    arguments: Vec<OsString>,
    inherit_claude_model: bool,
) -> Result<i32> {
    reject_model_override(&arguments)?;
    let config = ServiceConfig::new(options)?;
    // Reject invalid launch policy before creating a reusable daemon.
    let policy_header = policy::active_header()?;
    let cwd = std::env::current_dir().context("resolve Claude Code working directory")?;
    let arguments = prepare_arguments(arguments, &cwd);
    let _session_lock = acquire_resume_session_lock(&config, &arguments)?;
    let base_url = ensure::run(&config, ensure::Mode::Ensure).await?;
    let session_id = session_id_for_launch(&arguments, || {
        format!("session_{}", Uuid::new_v4().simple())
    });
    let program = std::env::var_os("CLAUDEX_CLAUDE_PROGRAM").unwrap_or_else(|| "claude".into());
    let custom_headers = working_directory::custom_headers(
        std::env::var_os("ANTHROPIC_CUSTOM_HEADERS").as_deref(),
        &cwd,
        policy_header.as_deref(),
    );
    let mut command = Command::new(program);
    let isolated = claude_process::configure(&mut command);
    policy::apply_snapshot(&mut command, &policy_header);
    if !inherit_claude_model {
        command.args(["--model", &config.options.model]);
    }
    let mut child = ClaudeProcess::new(
        command
            .args(arguments)
            .env("ANTHROPIC_BASE_URL", base_url)
            .env("ANTHROPIC_AUTH_TOKEN", &config.token)
            .env("CLAUDE_CODE_WEBSEARCH_USE_CCR_PROXY", "1")
            .env("CLAUDE_CODE_SESSION_ID", session_id)
            .env("CLAUDE_CODE_SESSION_ACCESS_TOKEN", &config.token)
            .env("ANTHROPIC_CUSTOM_HEADERS", custom_headers)
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("ANTHROPIC_MODEL")
            .env_remove("CLAUDE_CODE_USE_BEDROCK")
            .env_remove("CLAUDE_CODE_USE_FOUNDRY")
            .env_remove("CLAUDE_CODE_USE_VERTEX")
            .env_remove("CLAUDE_CODE_SUBAGENT_MODEL")
            .env_remove("CLAUDEX_ADAPTER_LISTEN")
            .env_remove("CLAUDEX_BACKEND")
            .env_remove("CLAUDEX_CLAUDE_PROGRAM")
            .env_remove("CLAUDEX_CODEX_PROGRAM")
            .env_remove("CLAUDEX_COLLABORATOR_MODEL")
            .env_remove("CLAUDEX_COPILOT_PROGRAM")
            .env_remove("CLAUDEX_GROK_PROGRAM")
            .env_remove("CLAUDEX_MODEL")
            .env_remove("CLAUDEX_SUBSCRIPTION_MAX_PROCESSES")
            .env_remove("CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES")
            .env_remove(crate::anthropic::SUBAGENT_HARD_TIMEOUT_ENV)
            .env_remove(crate::anthropic::LEGACY_SUBAGENT_RESPONSE_TIMEOUT_ENV)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped())
            .spawn()
            .context("start Claude Code")?,
        isolated,
    );
    let stderr = child.take_stderr().context("capture Claude Code stderr")?;
    let model = config.options.model;
    let relay = thread::spawn(move || relay_stderr(stderr, &model));
    let status = child.wait().context("wait for Claude Code")?;
    relay
        .join()
        .map_err(|_| anyhow::anyhow!("Claude Code stderr relay panicked"))??;
    Ok(exit_code(status))
}

#[cfg(test)]
use health::wait_until_ready_with;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "launcher_tests.rs"]
mod tests;
