use std::{
    ffi::OsString,
    process::{Command, Stdio},
    thread,
};

use anyhow::{Context, Result, bail};
use uuid::Uuid;

use super::claude_relay::{reject_model_override, relay_stderr};
use super::process_io::exit_code;
use super::resume::{prepare_arguments, session_id_for_launch};
use super::{
    AdapterOptions, ClaudeProcess, ServiceConfig, claude_process, ensure, launcher_lock,
    launcher_logs, resume, session_process,
};
use crate::{subagent_policy as policy, working_directory};

pub(super) fn acquire_resume_session_lock(
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

pub(super) fn resume_session_busy_message(resume_id: &str) -> String {
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
    let policy_header = policy::active_header();
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
