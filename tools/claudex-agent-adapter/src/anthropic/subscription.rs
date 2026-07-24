use std::{
    future::Future,
    path::{Path, PathBuf},
    process::Output,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use axum::{body::Body, http::Response};
use serde_json::{Value, json};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
    sync::{OwnedSemaphorePermit, Semaphore},
};

use super::{
    Bridge, MessagesRequest, Segment, Usage,
    agent_effort::AgentEffort,
    content::{anthropic_response, estimated_tokens, token_count},
    subscription_request::{subscription_request_cwd, subscription_request_prompt},
    subscription_stream::subscription_streaming_response,
};

#[cfg(test)]
pub(super) use super::subscription_request::cwd_from_system;
pub(super) use super::subscription_request::requested_tools;

pub(crate) const DEFAULT_MAX_PROCESSES: usize = 20;
pub(crate) const DEFAULT_TIMEOUT_MINUTES: u64 = 120;
const MAX_PROCESSES_ENV: &str = "CLAUDEX_SUBSCRIPTION_MAX_PROCESSES";
const TIMEOUT_MINUTES_ENV: &str = "CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES";
const OUTER_TOOL_BRIDGE_SETTINGS: &str = r#"{"hooks":{"PreToolUse":[{"matcher":".*","hooks":[{"type":"command","command":"exit 2"}]}]}}"#;
pub(super) struct SubscriptionOptions {
    pub(super) effort: Option<String>,
    pub(super) tools: Vec<String>,
    pub(super) cwd: Option<PathBuf>,
    pub(super) slots: Arc<Semaphore>,
    pub(super) timeout: Duration,
    pub(super) tool_context: Option<SubscriptionToolContext>,
}

#[derive(Clone)]
pub(super) struct SubscriptionToolContext {
    pub(super) agent_efforts: Arc<super::agent_effort::AgentEffortIntents>,
    pub(super) client_user_id: Option<String>,
    pub(super) parent_model: String,
    pub(super) user_messages: Vec<Value>,
}

impl SubscriptionOptions {
    pub(super) fn internal(slots: Arc<Semaphore>, timeout: Duration) -> Self {
        Self {
            effort: None,
            tools: Vec::new(),
            cwd: None,
            slots,
            timeout,
            tool_context: None,
        }
    }
}

pub(super) struct SubscriptionLimits {
    pub(super) max_processes: usize,
    pub(super) timeout: Duration,
}

impl SubscriptionLimits {
    pub(crate) fn new(max_processes: usize, timeout_minutes: u64) -> Result<Self> {
        if max_processes == 0 || max_processes > Semaphore::MAX_PERMITS {
            bail!("subscription process limit is out of range");
        }
        let timeout_seconds = timeout_minutes
            .checked_mul(60)
            .filter(|seconds| *seconds > 0)
            .context("subscription timeout is out of range")?;
        Ok(Self {
            max_processes,
            timeout: Duration::from_secs(timeout_seconds),
        })
    }
}

pub(super) fn subscription_limits() -> SubscriptionLimits {
    subscription_limits_from(|name| std::env::var(name).ok())
}

pub(super) fn subscription_limits_from(get: impl Fn(&str) -> Option<String>) -> SubscriptionLimits {
    let max_processes = positive_usize(get(MAX_PROCESSES_ENV)).unwrap_or(DEFAULT_MAX_PROCESSES);
    let timeout_seconds = positive_u64(get(TIMEOUT_MINUTES_ENV))
        .and_then(|minutes| minutes.checked_mul(60))
        .unwrap_or(DEFAULT_TIMEOUT_MINUTES * 60);
    SubscriptionLimits {
        max_processes,
        timeout: Duration::from_secs(timeout_seconds),
    }
}

fn positive_usize(value: Option<String>) -> Option<usize> {
    value?
        .parse()
        .ok()
        .filter(|value| *value > 0 && *value <= Semaphore::MAX_PERMITS)
}

fn positive_u64(value: Option<String>) -> Option<u64> {
    value?.parse().ok().filter(|value| *value > 0)
}

impl Bridge {
    pub(super) fn claude_setting(&self, key: &str) -> Option<String> {
        self.settings_path
            .as_deref()
            .and_then(|path| setting_at(path, key))
    }

    pub(super) fn claude_collaborator_model(&self) -> Option<String> {
        self.claude_setting("model")
    }

    pub(super) fn claude_effort(&self) -> Option<String> {
        self.claude_setting("effortLevel")
            .filter(|effort| valid_effort(effort))
    }

    pub(super) async fn subscription_messages(
        &self,
        request: MessagesRequest,
        effort: Option<String>,
    ) -> Result<Response<Body>> {
        let input_tokens = u64::try_from(token_count(&request)).unwrap_or(u64::MAX);
        let options = self.subscription_options(&request, effort);
        let prompt = subscription_request_prompt(&request);
        if request.stream {
            return Ok(subscription_streaming_response(
                self.subscription_program.clone(),
                request.model,
                prompt,
                input_tokens,
                options,
            ));
        }
        let text =
            run_subscription_model(&self.subscription_program, &request.model, &prompt, options)
                .await?;
        let segment = Segment {
            blocks: vec![json!({"type":"text", "text":text})],
            stop_reason: "end_turn",
            usage: Usage {
                input_tokens,
                output_tokens: estimated_tokens(&text),
            },
        };
        Ok(anthropic_response(segment, &request.model))
    }

    pub(super) fn resolve_request_effort(
        &self,
        request: &MessagesRequest,
        agent_effort: AgentEffort,
    ) -> Option<String> {
        match agent_effort {
            AgentEffort::Explicit(effort) => Some(effort),
            AgentEffort::ConfiguredDefault => self.claude_effort(),
            AgentEffort::Unmatched => request_effort(&request.output_config)
                .map(str::to_owned)
                .or_else(|| self.claude_effort()),
        }
    }

    fn subscription_options(
        &self,
        request: &MessagesRequest,
        effort: Option<String>,
    ) -> SubscriptionOptions {
        SubscriptionOptions {
            effort,
            tools: requested_tools(&request.tools),
            cwd: subscription_request_cwd(request),
            slots: Arc::clone(&self.subscription_slots),
            timeout: self.subscription_timeout,
            tool_context: Some(SubscriptionToolContext {
                agent_efforts: Arc::clone(&self.agent_efforts),
                client_user_id: request
                    .metadata
                    .get("user_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                parent_model: request.model.clone(),
                user_messages: request.messages.clone(),
            }),
        }
    }
}

pub(super) fn request_effort(output_config: &Value) -> Option<&str> {
    output_config
        .get("effort")
        .and_then(Value::as_str)
        .filter(|effort| valid_effort(effort))
}

pub(super) fn valid_effort(effort: &str) -> bool {
    matches!(effort, "low" | "medium" | "high" | "xhigh" | "max")
}

pub(super) fn claude_settings_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude/settings.json"))
}

pub(super) fn setting_at(path: &Path, key: &str) -> Option<String> {
    let settings = std::fs::read(path).ok()?;
    serde_json::from_slice::<Value>(&settings)
        .ok()?
        .get(key)?
        .as_str()
        .filter(|model| !model.is_empty())
        .map(str::to_owned)
}

pub(super) fn subscription_prompt(tool: &str, arguments: &Value, transcript: &[Value]) -> String {
    if tool == "advisor" {
        return format!(
            "Act as a rigorous advisor. Review the complete conversation below and return concise, actionable guidance to the primary coding agent. Do not use tools.\n\n{}",
            serde_json::to_string(transcript).unwrap_or_default()
        );
    }
    format!(
        "Work as an independent Claude collaborator. Complete the delegated task using the supplied conversation context. Do not use tools.\n\nTask:\n{}\n\nConversation:\n{}",
        arguments
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or("Review the conversation and suggest the next step."),
        serde_json::to_string(transcript).unwrap_or_default()
    )
}

pub(super) async fn run_subscription_model(
    program: &Path,
    model: &str,
    prompt: &str,
    options: SubscriptionOptions,
) -> Result<String> {
    let _permit = acquire_subscription_slot(Arc::clone(&options.slots), options.timeout).await?;
    let mut command = subscription_command(program, model, &options, OutputMode::Json);
    let mut child = spawn_subscription(&mut command, model)?;
    write_subscription_prompt(&mut child, prompt).await?;
    let output = wait_for_subscription(child.wait_with_output(), options.timeout).await?;
    if !output.status.success() {
        bail!(
            "Claude subscription model {model} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    subscription_result(&output.stdout)
}

pub(super) async fn acquire_subscription_slot(
    slots: Arc<Semaphore>,
    timeout: Duration,
) -> Result<OwnedSemaphorePermit> {
    tokio::time::timeout(timeout, slots.acquire_owned())
        .await
        .map_err(|_| anyhow!("Claude subscription capacity wait timed out"))?
        .map_err(|_| anyhow!("Claude subscription capacity is closed"))
}

pub(super) fn spawn_subscription(command: &mut Command, model: &str) -> Result<Child> {
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to start Claude subscription model {model}"))
}

pub(super) async fn write_subscription_prompt(child: &mut Child, prompt: &str) -> Result<()> {
    child
        .stdin
        .take()
        .context("Claude subscription stdin is unavailable")?
        .write_all(prompt.as_bytes())
        .await
        .map_err(Into::into)
}

pub(super) async fn wait_for_subscription<F>(future: F, timeout: Duration) -> Result<Output>
where
    F: Future<Output = std::io::Result<Output>>,
{
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| anyhow!("Claude subscription timed out after {timeout:?}"))?
        .map_err(Into::into)
}

#[derive(Clone, Copy)]
pub(super) enum OutputMode {
    Json,
    StreamJson,
}

pub(super) fn subscription_command(
    program: &Path,
    model: &str,
    options: &SubscriptionOptions,
    output: OutputMode,
) -> Command {
    let mut command = Command::new(program);
    let tools = options.tools.join(",");
    let output_format = match output {
        OutputMode::Json => "json",
        OutputMode::StreamJson => "stream-json",
    };
    command.args([
        "--print",
        "--model",
        model,
        "--output-format",
        output_format,
        "--tools",
        &tools,
        "--no-session-persistence",
    ]);
    if !options.tools.is_empty() {
        command.args(["--allowedTools", &tools]);
    }
    if matches!(output, OutputMode::StreamJson) {
        command.args(["--include-partial-messages", "--verbose"]);
        if !options.tools.is_empty() {
            command.args(["--settings", OUTER_TOOL_BRIDGE_SETTINGS]);
        }
    }
    if let Some(effort) = &options.effort {
        command.args(["--effort", effort]);
    }
    if let Some(cwd) = &options.cwd {
        command.current_dir(cwd);
    }
    remove_proxy_environment(&mut command);
    command
}

fn remove_proxy_environment(command: &mut Command) {
    for variable in [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_MODEL",
        "CLAUDE_CODE_ENABLE_EXPERIMENTAL_ADVISOR_TOOL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
        "ENABLE_CLAUDEAI_MCP_SERVERS",
    ] {
        command.env_remove(variable);
    }
}

pub(super) fn subscription_result(stdout: &[u8]) -> Result<String> {
    let value: Value =
        serde_json::from_slice(stdout).context("Claude subscription returned invalid JSON")?;
    validate_subscription_result(&value)?;
    value
        .get("result")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("Claude subscription JSON did not contain a result: {value}"))
}

pub(super) fn validate_subscription_result(result: &Value) -> Result<()> {
    if result.get("is_error").and_then(Value::as_bool) == Some(true)
        || result.get("subtype").and_then(Value::as_str) != Some("success")
    {
        bail!(
            "Claude subscription failed: {}",
            result.get("result").unwrap_or(result)
        );
    }
    Ok(())
}
