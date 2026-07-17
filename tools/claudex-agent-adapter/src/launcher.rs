use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{ADAPTER_PROTOCOL_VERSION, agent_backend::BackendRoute};

const LOCAL_TOKEN: &str = "claudex-local";
const START_ATTEMPTS: usize = 40;

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
    model: String,
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
            && health.model == self.options.model
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

fn process_matches(pid: u32, executable: &Path) -> bool {
    let Ok(output) = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
    else {
        return false;
    };
    let command = String::from_utf8_lossy(&output.stdout);
    command.contains(&executable.to_string_lossy().to_string()) && command.contains("serve")
}

fn terminate(pid: u32) {
    let _status = Command::new("kill")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
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
    wait_until_ready_with(client, config, START_ATTEMPTS, Duration::from_millis(250)).await
}

async fn wait_until_ready_with(
    client: &reqwest::Client,
    config: &ServiceConfig,
    attempts: usize,
    delay: Duration,
) -> Result<()> {
    for _ in 0..attempts {
        if fetch_health(client, config)
            .await
            .is_some_and(|health| config.matches(&health))
        {
            return Ok(());
        }
        tokio::time::sleep(delay).await;
    }
    bail!(
        "agent adapter failed to start; see {}",
        config.log_path.display()
    )
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::agent_backend::BackendKind;

    fn config() -> ServiceConfig {
        ServiceConfig {
            options: AdapterOptions {
                routes: vec![BackendRoute {
                    model: "test-model".to_owned(),
                    backend: BackendKind::CodexAppServer,
                }],
                listen: "127.0.0.1:8318".parse().expect("default listen"),
                model: "test-model".to_owned(),
                subscription_max_processes: 20,
                subscription_timeout_minutes: 120,
            },
            token: LOCAL_TOKEN.to_owned(),
            executable: PathBuf::from("/tmp/adapter"),
            log_path: PathBuf::from("/tmp/adapter.log"),
        }
    }

    #[test]
    fn formats_the_listener_and_matches_all_health_settings() {
        let config = config();
        assert_eq!(config.base_url(), "http://127.0.0.1:8318");
        assert!(config.matches(&healthy(&config)));
    }

    #[test]
    fn connects_to_loopback_for_unspecified_bind_addresses() {
        let mut config = config();
        config.options.listen = "0.0.0.0:9000".parse().expect("IPv4 listener");
        assert_eq!(config.base_url(), "http://127.0.0.1:9000");
        config.options.listen = "[::]:9000".parse().expect("IPv6 listener");
        assert_eq!(config.base_url(), "http://[::1]:9000");
    }

    #[test]
    fn rejects_a_second_main_model_argument() {
        assert!(reject_model_override(&["--model".into(), "other".into()]).is_err());
        assert!(reject_model_override(&["--model=other".into()]).is_err());
        assert!(reject_model_override(&["--continue".into()]).is_ok());
    }

    fn healthy(config: &ServiceConfig) -> Health {
        Health {
            status: "ok".to_owned(),
            pid: Some(42),
            protocol_version: ADAPTER_PROTOCOL_VERSION,
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            backend_routes: route_descriptions(&config.options.routes),
            model: config.options.model.clone(),
            subscription_max_processes: 20,
            subscription_timeout_minutes: 120,
        }
    }

    #[test]
    fn rejects_each_stale_health_dimension() {
        let config = config();
        let mut stale = Vec::new();
        let mut health = healthy(&config);
        health.status = "unavailable".to_owned();
        stale.push(health);
        let mut health = healthy(&config);
        health.protocol_version += 1;
        stale.push(health);
        let mut health = healthy(&config);
        health.build_id = "stale".to_owned();
        stale.push(health);
        let mut health = healthy(&config);
        health.model = "other".to_owned();
        stale.push(health);
        let mut health = healthy(&config);
        health.subscription_max_processes = 7;
        stale.push(health);
        let mut health = healthy(&config);
        health.subscription_timeout_minutes = 45;
        stale.push(health);
        for health in stale {
            assert!(!config.matches(&health));
        }
    }

    #[test]
    fn relays_non_warning_stderr_bytes() {
        let mut output = Vec::new();
        let advisor_warning = "Advisor disabled — base model 'test-model' has no advisor rank\n";
        let connector_warning =
            "claude.ai connectors are disabled because another auth source takes precedence\n";
        let input = format!("{advisor_warning}{connector_warning}kept warning\n");
        relay_filtered(input.as_bytes(), "test-model", &mut output).expect("relay fixture");
        assert_eq!(output, b"kept warning\n");
    }

    #[cfg(unix)]
    #[test]
    fn converts_signal_exit_statuses() {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(exit_code(std::process::ExitStatus::from_raw(9)), 137);
        assert_eq!(exit_code(std::process::ExitStatus::from_raw(0)), 0);
    }

    #[tokio::test]
    async fn handles_absent_legacy_processes_and_readiness_timeout() {
        let mut config = config();
        config.options.listen = "127.0.0.1:1".parse().expect("closed test listener");
        config.executable = PathBuf::from("/definitely/missing/adapter");
        stop_stale(&config, None).await;
        terminate(u32::MAX);
        let error = wait_until_ready_with(
            &reqwest::Client::new(),
            &config,
            1,
            Duration::from_millis(1),
        )
        .await
        .expect_err("unreachable adapter must time out");
        assert!(error.to_string().contains("failed to start"));
        stop_stale(&config, Some(std::process::id())).await;
    }

    #[test]
    fn reports_adapter_log_configuration_errors() {
        let mut config = config();
        config.log_path = PathBuf::new();
        let error = start_adapter(&config).expect_err("parentless log path must fail");
        assert!(error.to_string().contains("adapter log has no parent"));
    }
}
