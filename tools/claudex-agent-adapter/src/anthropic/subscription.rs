use anyhow::{Context, Result, anyhow, bail};
use axum::{body::Body, http::Response};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, ChildStdin, Command},
    sync::{OwnedSemaphorePermit, Semaphore},
};

mod effort;
pub(super) mod failure;
mod lifecycle;
mod options;
mod retry;
#[cfg(test)]
pub(super) use effort::request_effort;
pub(super) use effort::valid_effort;

#[cfg(test)]
pub(super) use super::subscription_request::cwd_from_system;
#[cfg(test)]
pub(super) use super::subscription_request::requested_tools;
pub(super) use super::subscription_request::requested_tools_for_request;
use super::{
    Bridge, MessagesRequest, Segment, Usage, WebEvidenceSummary,
    content::{anthropic_response, estimated_tokens, token_count},
    subscription_request::{
        is_compaction_request, request_json_schema, subscription_request_cwd,
        subscription_request_prompt,
    },
    subscription_stream::{
        ACTIVITY_KEEPALIVE_INTERVAL, INITIAL_ACTIVITY_DELAY, subscription_streaming_response,
    },
};
use crate::NONINTERACTIVE_CHILD_ENV;
use failure::{
    local_failure, local_result, process_failure, spawn_child, subscription_result_for_model,
    timeout_failure,
};
pub(super) use failure::{
    subscription_failure, subscription_result_text, validate_subscription_result_for_model,
};
#[cfg(test)]
pub(super) use failure::{subscription_result, validate_subscription_result};
#[cfg(test)]
pub(in crate::anthropic) use lifecycle::terminate_subscription;
pub(in crate::anthropic) use lifecycle::terminate_subscription_process_group;
use lifecycle::{collect_subscription_output, terminate_after_subscription_failure};
pub(super) use options::{
    DEFAULT_STDERR_DRAIN_GRACE, DEFAULT_TERMINATION_TIMEOUT, SubscriptionOptions,
    SubscriptionToolContext,
};
pub(super) use retry::with_transient_retries;
#[cfg(test)]
pub(super) use retry::{should_retry_subscription, transient_retry_delay};
pub(crate) const DEFAULT_MAX_PROCESSES: usize = 20;
pub(crate) const DEFAULT_TIMEOUT_MINUTES: u64 = 120;
const MAX_PROCESSES_ENV: &str = "CLAUDEX_SUBSCRIPTION_MAX_PROCESSES";
const TIMEOUT_MINUTES_ENV: &str = "CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES";
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
        is_subagent: bool,
        tools_were_provided: bool,
    ) -> Result<Response<Body>> {
        let input_tokens = u64::try_from(token_count(&request)).unwrap_or(u64::MAX);
        let options = self.subscription_options(&request, effort, is_subagent, tools_were_provided);
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
                web_search_requests: 0,
            },
            web_evidence: WebEvidenceSummary::default(),
        };
        Ok(anthropic_response(segment, &request.model))
    }

    fn subscription_options(
        &self,
        request: &MessagesRequest,
        effort: Option<String>,
        is_subagent: bool,
        tools_were_provided: bool,
    ) -> SubscriptionOptions {
        SubscriptionOptions {
            effort,
            tools: requested_tools_for_request(request, !is_subagent),
            disable_tools: (tools_were_provided && request.tools.is_empty())
                || is_compaction_request(request),
            json_schema: request_json_schema(&request.output_config),
            cwd: subscription_request_cwd(request),
            slots: Arc::clone(&self.subscription_slots),
            timeout: self.subscription_timeout,
            initial_activity_delay: INITIAL_ACTIVITY_DELAY,
            activity_keepalive_interval: ACTIVITY_KEEPALIVE_INTERVAL,
            stderr_drain_grace: DEFAULT_STDERR_DRAIN_GRACE,
            termination_timeout: DEFAULT_TERMINATION_TIMEOUT,
            tool_context: Some(SubscriptionToolContext {
                agent_efforts: Arc::clone(&self.agent_efforts),
                model_catalog: self.model_catalog.clone(),
                client_user_id: request
                    .metadata
                    .get("user_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                parent_model: request.model.clone(),
                user_messages: request.messages.clone(),
                system: request.system.clone(),
            }),
        }
    }
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

#[cfg(test)]
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
    retry::with_transient_retries(model, || {
        run_subscription_model_attempt(program, model, prompt, &options)
    })
    .await
}

async fn run_subscription_model_attempt(
    program: &Path,
    model: &str,
    prompt: &str,
    options: &SubscriptionOptions,
) -> Result<String> {
    let _permit = acquire_subscription_slot(Arc::clone(&options.slots), options.timeout).await?;
    let mut command = subscription_command(program, model, options, OutputMode::Json);
    let (mut child, stdin) = spawn_child(&mut command, model)?;
    let interaction = collect_subscription_output(&mut child, stdin, prompt);
    let (prompt_result, output) = match tokio::time::timeout(options.timeout, interaction).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            let error = local_failure(model, "failed to collect child output", &error);
            return terminate_after_subscription_failure(&mut child, error).await;
        }
        Err(_) => {
            let error = timeout_failure(model, options.timeout);
            return terminate_after_subscription_failure(&mut child, error).await;
        }
    };
    if !output.status.success() {
        return Err(process_failure(
            model,
            &output.status,
            &output.stdout,
            &output.stderr,
        ));
    }
    local_result(model, "failed to write prompt", prompt_result)?;
    subscription_result_for_model(&output.stdout, Some(model))
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

pub(super) fn take_subscription_stdin(child: &mut Child) -> Result<ChildStdin> {
    child
        .stdin
        .take()
        .context("Claude subscription stdin is unavailable")
}

pub(super) async fn write_subscription_prompt(mut stdin: ChildStdin, prompt: &str) -> Result<()> {
    stdin.write_all(prompt.as_bytes()).await?;
    stdin.shutdown().await.map_err(Into::into) // explicit EOF for --print
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
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.as_std_mut().process_group(0);
    }
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
        "--no-session-persistence",
    ]);
    if options.disable_tools {
        command.args(["--safe-mode", "--tools", "", "--allowedTools", ""]);
    } else if !options.tools.is_empty() {
        let tools = options.tools.join(",");
        command.args(["--tools", &tools]);
        command.args(["--allowedTools", &tools]);
    }
    if let Some(schema) = &options.json_schema {
        command.args(["--json-schema", schema]);
    }
    if matches!(output, OutputMode::StreamJson) {
        command.args(["--include-partial-messages", "--verbose"]);
    }
    if let Some(effort) = &options.effort {
        command.args(["--effort", effort]);
    }
    if let Some(cwd) = &options.cwd {
        command.current_dir(cwd);
    }
    command.env(NONINTERACTIVE_CHILD_ENV, "1");
    remove_proxy_environment(&mut command);
    command
}

fn remove_proxy_environment(command: &mut Command) {
    for variable in [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_MODEL",
        // Isolated claudex CLAUDE_CONFIG_DIR has no OAuth; subscription
        // children must use the real ~/.claude login instead.
        "CLAUDE_CONFIG_DIR",
        "CLAUDEX_ACTIVE",
        "CLAUDE_CODE_ENABLE_EXPERIMENTAL_ADVISOR_TOOL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
        "ENABLE_CLAUDEAI_MCP_SERVERS",
    ] {
        command.env_remove(variable);
    }
    crate::web_search::clear_local_ccr_environment(command);
}
