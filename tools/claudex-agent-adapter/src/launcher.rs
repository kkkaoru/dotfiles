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

use crate::{ADAPTER_PROTOCOL_VERSION, agent_backend::BackendRoute};

mod daemon_process;

use daemon_process::{matches as process_matches, terminate};

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
}

#[derive(Debug)]
struct ServiceConfig {
    options: AdapterOptions,
    token: String,
    executable: PathBuf,
    log_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct Health {
    status: String,
    pid: Option<u32>,
    protocol_version: u64,
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
        Ok(Self {
            options,
            token,
            executable,
            log_path: cache.join("adapter.log"),
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
        health.status == "ok"
            && health.protocol_version == ADAPTER_PROTOCOL_VERSION
            && health.build_id == env!("CLAUDEX_BUILD_ID")
            && health.backend_routes == route_descriptions(&self.options.routes)
            && health.subscription_max_processes == self.options.subscription_max_processes
            && health.subscription_timeout_minutes == self.options.subscription_timeout_minutes
    }
}

pub async fn ensure_running(options: AdapterOptions) -> Result<String> {
    let config = ServiceConfig::new(options)?;
    ensure_config_running(&config).await
}

pub async fn run_claude(options: AdapterOptions, arguments: Vec<OsString>) -> Result<i32> {
    reject_model_override(&arguments)?;
    let config = ServiceConfig::new(options)?;
    let base_url = ensure_config_running(&config).await?;
    let program = std::env::var_os("CLAUDEX_CLAUDE_PROGRAM").unwrap_or_else(|| "claude".into());
    let mut child = Command::new(program)
        .arg("--model")
        .arg(&config.options.model)
        .args(arguments)
        .env("ANTHROPIC_BASE_URL", base_url)
        .env("ANTHROPIC_AUTH_TOKEN", &config.token)
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
    let client = reqwest::Client::new();
    if let Some(health) = fetch_health(&client, config).await {
        if config.matches(&health) && authenticates(&client, config).await {
            return Ok(config.base_url());
        }
        stop_stale(config, health.pid).await;
    }
    start_adapter(config)?;
    wait_until_ready(&client, config).await?;
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

fn relay_filtered(input: impl std::io::Read, model: &str, output: &mut impl Write) -> Result<()> {
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

async fn stop_stale(config: &ServiceConfig, pid: Option<u32>) {
    if let Some(pid) = pid
        && pid != std::process::id()
        && process_matches(pid, &config.executable)
    {
        terminate(pid);
    }
    let client = reqwest::Client::new();
    for _ in 0..20 {
        if fetch_health(&client, config).await.is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn start_adapter(config: &ServiceConfig) -> Result<()> {
    let log_dir = config
        .log_path
        .parent()
        .context("adapter log has no parent")?;
    fs::create_dir_all(log_dir).context("create adapter log directory")?;
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config.log_path)
        .context("open adapter log")?;
    let stderr = stdout.try_clone().context("clone adapter log handle")?;
    Command::new("nohup")
        .arg(&config.executable)
        .args(daemon_arguments(&config.options))
        .env("ANTHROPIC_AUTH_TOKEN", &config.token)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("ANTHROPIC_MODEL")
        .env_remove("CLAUDE_CODE_SUBAGENT_MODEL")
        .env_remove("CLAUDE_CODE_USE_BEDROCK")
        .env_remove("CLAUDE_CODE_USE_FOUNDRY")
        .env_remove("CLAUDE_CODE_USE_VERTEX")
        .env_remove("CLAUDEX_ADAPTER_LISTEN")
        .env_remove("CLAUDEX_CLAUDE_PROGRAM")
        .env_remove("CLAUDEX_MODEL")
        .env_remove("CLAUDEX_SUBSCRIPTION_MAX_PROCESSES")
        .env_remove("CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .context("start adapter daemon")?;
    Ok(())
}

fn daemon_arguments(options: &AdapterOptions) -> Vec<OsString> {
    let mut arguments = vec![
        "serve".into(),
        "--model".into(),
        options.model.clone().into(),
    ];
    for route in &options.routes {
        arguments.push("--backend-route".into());
        arguments.push(format!("{}={}", route.model, route.backend).into());
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
    routes
        .iter()
        .map(|route| format!("{}={}", route.model, route.backend))
        .collect()
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
        if fetch_health(client, config)
            .await
            .is_some_and(|health| config.matches(&health))
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
