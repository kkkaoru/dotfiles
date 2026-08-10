use std::{
    collections::hash_map::DefaultHasher,
    ffi::OsString,
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Write},
    net::SocketAddr,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use uuid::Uuid;

mod claude_process;
mod daemon_arguments;
mod daemon_process;
mod daemon_start;
mod ensure;
mod fallback;
mod handover;
mod health;
mod live;
mod promote;
pub(crate) use live::{
    RETAINED_STATE_ENV, RetainedGeneration, SERVICE_LISTEN_ENV, load_retained_from_env,
    read_retained,
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
use crate::{
    ADAPTER_PROTOCOL_VERSION, agent_backend::BackendRoute, app_server, subagent_policy as policy,
    working_directory,
};
use claude_process::ClaudeProcess;
#[cfg(test)]
use daemon_arguments::{daemon_arguments, hot_swap_wait_arguments};
use daemon_arguments::{
    route_descriptions, search_worker_route_descriptions, worker_route_descriptions,
};
#[cfg(test)]
use daemon_start::start_adapter;
#[cfg(test)]
use health::Health;
use health::{authenticates, fetch_health, wait_until_ready, wait_until_recovery_ready};
use resume::{prepare_arguments, session_id_for_launch};

const LOCAL_TOKEN: &str = "claudex-local";
#[cfg(not(test))]
const START_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
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

#[derive(Debug)]
struct ServiceConfig {
    options: AdapterOptions,
    token: String,
    codex_config_fingerprint: String,
    service_config_fingerprint: String,
    executable: PathBuf,
    log_path: PathBuf,
    lock_path: PathBuf,
}

impl ServiceConfig {
    fn new(options: AdapterOptions) -> Result<Self> {
        let token = std::env::var("ANTHROPIC_AUTH_TOKEN")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| LOCAL_TOKEN.to_owned());
        if requires_authentication(&options.listen, &token) {
            bail!("ANTHROPIC_AUTH_TOKEN is required for a non-loopback listener");
        }
        let executable = installed_adapter::resolve_service_executable(
            std::env::current_exe().context("locate adapter executable")?,
        );
        let home = std::env::var_os("HOME").context("HOME is required")?;
        let source_home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(&home).join(".codex"));
        let codex_config_fingerprint = app_server::provider_config_fingerprint(&source_home);
        let service_config_fingerprint =
            service_config_fingerprint(&options, &codex_config_fingerprint);
        let cache = PathBuf::from(home).join(".cache/claudex");
        let log_path = launcher_logs::adapter_log_path(&cache, &options.listen);
        let lock_path = launcher_logs::adapter_lock_path(&cache, &options.listen);
        Ok(Self {
            options,
            token,
            codex_config_fingerprint,
            service_config_fingerprint,
            executable,
            log_path,
            lock_path,
        })
    }

    fn base_url(&self) -> String {
        let listen = match self.options.listen {
            SocketAddr::V4(address) if address.ip().is_unspecified() => {
                SocketAddr::from(([127, 0, 0, 1], address.port()))
            }
            SocketAddr::V6(address) if address.ip().is_unspecified() => {
                SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], address.port()))
            }
            listen => listen,
        };
        format!("http://{listen}")
    }

    fn with_listen(&self, listen: SocketAddr) -> Self {
        let cache = self.log_path.parent().expect("adapter log parent");
        let mut options = self.options.clone();
        options.listen = listen;
        Self {
            options,
            token: self.token.clone(),
            codex_config_fingerprint: self.codex_config_fingerprint.clone(),
            service_config_fingerprint: self.service_config_fingerprint.clone(),
            executable: self.executable.clone(),
            log_path: launcher_logs::adapter_log_path(cache, &listen),
            lock_path: launcher_logs::adapter_lock_path(cache, &listen),
        }
    }
}

fn service_config_fingerprint(options: &AdapterOptions, codex_fingerprint: &str) -> String {
    let mut hasher = DefaultHasher::new();
    ADAPTER_PROTOCOL_VERSION.hash(&mut hasher);
    codex_fingerprint.hash(&mut hasher);
    options.model.hash(&mut hasher);
    route_descriptions(&options.routes).hash(&mut hasher);
    worker_route_descriptions(&options.model_catalog).hash(&mut hasher);
    search_worker_route_descriptions(&options.model_catalog).hash(&mut hasher);
    program_identity::identity(&options.routes).hash(&mut hasher);
    options.subscription_max_processes.hash(&mut hasher);
    options.subscription_timeout_minutes.hash(&mut hasher);
    options
        .subagent_hard_timeout_seconds
        .map(std::num::NonZeroU64::get)
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub(crate) fn recovery_generation() -> Option<String> {
    recovery_manifest::generation_from_environment()
}

pub async fn ensure_running(options: AdapterOptions) -> Result<String> {
    let config = ServiceConfig::new(options)?;
    ensure::run(&config, ensure::Mode::Ensure).await
}

/// Run macOS notify dedupe/delivery in the on-disk install binary.
pub fn run_internal_notify(arguments: Vec<OsString>) -> Result<()> {
    macos_notify_dispatch::run_internal(arguments)
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
    let _session_lock = if let Some(resume_id) = resume::session_lock_id(&arguments) {
        if session_process::another_resume_launcher_is_active(&resume_id)? {
            bail!(
                "resume session '{resume_id}' is already active; continue in the existing Claude Code process or use --fork-session"
            );
        }
        let cache = config
            .log_path
            .parent()
            .context("adapter log has no parent directory")?;
        let path = launcher_logs::session_lock_path(cache, &resume_id);
        Some(launcher_lock::try_acquire(&path)?.ok_or_else(|| {
            anyhow::anyhow!(
                "resume session '{resume_id}' is already active; continue in the existing Claude Code process or use --fork-session"
            )
        })?)
    } else {
        None
    };
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

fn requires_authentication(listen: &SocketAddr, token: &str) -> bool {
    !listen.ip().is_loopback() && token == LOCAL_TOKEN
}

fn reject_model_override(arguments: &[OsString]) -> Result<()> {
    if arguments.iter().any(|argument| {
        argument
            .to_str()
            .is_some_and(|argument| argument == "--model" || argument.starts_with("--model="))
    }) {
        bail!("pass the main model to adapter option --model, not to Claude Code arguments");
    }
    Ok(())
}

fn relay_stderr(stderr: impl std::io::Read, model: &str) -> Result<()> {
    let mut output = std::io::stderr().lock();
    relay_filtered(stderr, model, &mut output)
}

fn relay_filtered(
    mut input: impl std::io::Read,
    model: &str,
    output: &mut impl Write,
) -> Result<()> {
    relay_filtered_io(&mut input, model, output)
}

fn relay_filtered_io(
    input: &mut dyn std::io::Read,
    model: &str,
    output: &mut dyn Write,
) -> Result<()> {
    let advisor_warning = format!("Advisor disabled — base model '{model}' has no advisor rank");
    let connector_warning = "claude.ai connectors are disabled because";
    let mut reader = BufReader::new(input);
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line)? > 0 {
        let text = String::from_utf8_lossy(&line);
        if !text.contains(&advisor_warning) && !text.contains(connector_warning) {
            output.write_all(&line)?;
            output.flush()?;
        }
        line.clear();
    }
    Ok(())
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or_else(|| {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            status.signal().map_or(1, |signal| 128 + signal)
        }
        #[cfg(not(unix))]
        {
            1
        }
    })
}

#[cfg(test)]
use health::wait_until_ready_with;

#[cfg(test)]
include!("launcher_tests.rs");
