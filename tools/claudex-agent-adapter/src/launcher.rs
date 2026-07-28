use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    net::SocketAddr,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

mod daemon_process;
mod handover;
mod launcher_lock;
mod launcher_logs;
use crate::{
    ADAPTER_PROTOCOL_VERSION, agent_backend::BackendRoute, subagent_policy as policy,
    working_directory,
};
use handover::ServiceState;

const LOCAL_TOKEN: &str = "claudex-local";
const START_TIMEOUT: Duration = Duration::from_secs(10);
const START_INITIAL_POLL_DELAY: Duration = Duration::from_millis(10);
const START_MAX_POLL_DELAY: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct AdapterOptions {
    pub routes: Vec<BackendRoute>,
    pub model: String,
    pub listen: SocketAddr,
    pub subscription_max_processes: usize,
    pub subscription_timeout_minutes: u64,
    pub model_catalog: crate::provider_config::ModelCatalog,
}

#[derive(Debug)]
struct ServiceConfig {
    options: AdapterOptions,
    token: String,
    executable: PathBuf,
    log_path: PathBuf,
    lock_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct Health {
    status: String,
    pid: Option<u32>,
    protocol_version: u64,
    #[serde(rename = "build_id")]
    build_id: String,
    #[serde(default)]
    backend_routes: Vec<String>,
    subscription_max_processes: usize,
    subscription_timeout_minutes: u64,
}

impl ServiceConfig {
    fn new(options: AdapterOptions) -> Result<Self> {
        let token = std::env::var("ANTHROPIC_AUTH_TOKEN")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| LOCAL_TOKEN.to_owned());
        if !options.listen.ip().is_loopback() & (token == LOCAL_TOKEN) {
            bail!("ANTHROPIC_AUTH_TOKEN is required for a non-loopback listener");
        }
        let executable = std::env::current_exe().context("locate adapter executable")?;
        let cache = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is required")?
            .join(".cache/claudex");
        let log_path = launcher_logs::adapter_log_path(&cache, &options.listen);
        let lock_path = launcher_logs::adapter_lock_path(&cache, &options.listen);
        Ok(Self {
            options,
            token,
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

    fn matches(&self, health: &Health) -> bool {
        // Protocol/config compatibility is separate from build freshness. The handover
        // state machine checks the build ID without interrupting accepted responses.
        health.status == "ok"
            && health.protocol_version == ADAPTER_PROTOCOL_VERSION
            && health.backend_routes == route_descriptions(&self.options.routes)
            && health.subscription_max_processes == self.options.subscription_max_processes
            && health.subscription_timeout_minutes == self.options.subscription_timeout_minutes
    }
}

pub async fn ensure_running(options: AdapterOptions) -> Result<String> {
    let config = ServiceConfig::new(options)?;
    ensure_config_running(&config).await
}

pub async fn run_claude(
    options: AdapterOptions,
    arguments: Vec<OsString>,
    inherit_claude_model: bool,
) -> Result<i32> {
    reject_model_override(&arguments)?;
    let config = ServiceConfig::new(options)?;
    let base_url = ensure_config_running(&config).await?;
    let program = std::env::var_os("CLAUDEX_CLAUDE_PROGRAM").unwrap_or_else(|| "claude".into());
    let cwd = std::env::current_dir().context("resolve Claude Code working directory")?;
    let policy_header = policy::active_header()?;
    let custom_headers = working_directory::custom_headers(
        std::env::var_os("ANTHROPIC_CUSTOM_HEADERS").as_deref(),
        &cwd,
        policy_header.as_deref(),
    );
    let mut command = Command::new(program);
    policy::apply_snapshot(&mut command, &policy_header);
    if !inherit_claude_model {
        command.args(["--model", &config.options.model]);
    }
    let mut child = command
        .args(arguments)
        .env("ANTHROPIC_BASE_URL", base_url)
        .env("ANTHROPIC_AUTH_TOKEN", &config.token)
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
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
        .context("start Claude Code")?;
    let stderr = child.stderr.take().context("capture Claude Code stderr")?;
    let model = config.options.model;
    let relay = thread::spawn(move || relay_stderr(stderr, &model));
    let status = child.wait().context("wait for Claude Code")?;
    relay
        .join()
        .map_err(|_| anyhow::anyhow!("Claude Code stderr relay panicked"))??;
    Ok(exit_code(status))
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

async fn ensure_config_running(config: &ServiceConfig) -> Result<String> {
    let _lock = launcher_lock::acquire(&config.lock_path)?;
    let client = reqwest::Client::new();
    match handover::inspect_service(&client, config).await {
        ServiceState::Reuse => return Ok(config.base_url()),
        ServiceState::Replace(pid) => {
            handover::release_stale_listener(&client, config, pid).await?
        }
        ServiceState::Start => {}
    }
    let started_pid = start_adapter(config)?;
    if let Err(error) = wait_until_ready(&client, config).await {
        if daemon_process::matches(started_pid, &config.executable) {
            daemon_process::terminate(started_pid);
        }
        return Err(error);
    }
    Ok(config.base_url())
}

async fn authenticates(client: &reqwest::Client, config: &ServiceConfig) -> bool {
    client
        .get(format!("{}/v1/models", config.base_url()))
        .bearer_auth(&config.token)
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
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

async fn fetch_health(client: &reqwest::Client, config: &ServiceConfig) -> Option<Health> {
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

fn start_adapter(config: &ServiceConfig) -> Result<u32> {
    let log_dir = config
        .log_path
        .parent()
        .context("adapter log has no parent")?;
    fs::create_dir_all(log_dir).context("create adapter log directory")?;
    launcher_logs::archive_previous_log(&config.log_path)?;
    let mut stdout = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&config.log_path)
        .context("open adapter log")?;
    launcher_logs::write_adapter_log_header(
        &mut stdout,
        &config.options.model,
        &config.options.listen,
        config.token.len(),
    )?;
    let stdout = stdout;
    let stderr = stdout.try_clone().context("clone adapter log handle")?;
    let mut command = Command::new("nohup");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = crate::path_env::apply_daemon_env(
        command
            .arg(&config.executable)
            .args(daemon_arguments(&config.options)),
        &config.token,
    )
    .stdin(Stdio::null())
    .stdout(Stdio::from(stdout))
    .stderr(Stdio::from(stderr))
    .spawn()
    .context("start adapter daemon")?;
    Ok(child.id())
}

fn daemon_arguments(options: &AdapterOptions) -> Vec<OsString> {
    let mut arguments = vec![
        "serve".into(),
        "--model".into(),
        options.model.clone().into(),
    ];
    for route in &options.routes {
        arguments.push("--backend-route-json".into());
        arguments.push(
            serde_json::to_string(route)
                .expect("backend route must serialize")
                .into(),
        );
    }
    arguments.extend([
        "--listen".into(),
        options.listen.to_string().into(),
        "--subscription-max-processes".into(),
        options.subscription_max_processes.to_string().into(),
        "--subscription-timeout-minutes".into(),
        options.subscription_timeout_minutes.to_string().into(),
    ]);
    arguments
}

fn route_descriptions(routes: &[BackendRoute]) -> Vec<String> {
    routes.iter().map(BackendRoute::description).collect()
}

async fn wait_until_ready(client: &reqwest::Client, config: &ServiceConfig) -> Result<()> {
    wait_until_ready_with(
        client,
        config,
        START_TIMEOUT,
        START_INITIAL_POLL_DELAY,
        START_MAX_POLL_DELAY,
    )
    .await
}

async fn wait_until_ready_with(
    client: &reqwest::Client,
    config: &ServiceConfig,
    timeout: Duration,
    initial_delay: Duration,
    max_delay: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut delay = initial_delay;
    loop {
        if let Some(health) = fetch_health(client, config).await
            && config.matches(&health)
            && health.build_id == env!("CLAUDEX_BUILD_ID")
            && authenticates(client, config).await
        {
            return Ok(());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(delay.min(remaining)).await;
        delay = delay.saturating_mul(2).min(max_delay);
    }
    bail!(
        "agent adapter failed to start; see {}",
        config.log_path.display()
    )
}

#[cfg(test)]
include!("launcher_tests.rs");
