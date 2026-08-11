use std::{
    env,
    ffi::OsString,
    path::Path,
    process::{Command as StdCommand, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use agent_client_protocol::{self as acp, Agent as _};
use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::{process::Command, sync::oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use super::{client::AcpClient, plugin};
use crate::{app_server::events::ThreadEventDispatcher, path_env};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AcpProvider {
    Grok,
    Copilot,
    /// The model must be selected with ACP `set_session_model`.
    Configured,
    /// The configured command receives the model through a `{model}` argument.
    ConfiguredLaunchScoped,
}

impl AcpProvider {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Grok => "Grok",
            Self::Copilot => "Copilot",
            Self::Configured => "Configured",
            Self::ConfiguredLaunchScoped => "ConfiguredLaunch",
        }
    }

    pub(super) const fn driver_name(self) -> &'static str {
        match self {
            Self::Grok => "claudex-grok-acp",
            Self::Copilot => "claudex-copilot-acp",
            Self::Configured | Self::ConfiguredLaunchScoped => "claudex-configured-acp",
        }
    }

    pub(super) const fn model_is_launch_scoped(self) -> bool {
        matches!(self, Self::Grok | Self::ConfiguredLaunchScoped)
    }

    pub(super) const fn is_session_scoped_configured(self) -> bool {
        matches!(self, Self::Configured)
    }
}

pub(super) struct StartConnection<'a> {
    pub(super) provider: AcpProvider,
    pub(super) program: &'a OsString,
    pub(super) arguments: Option<&'a [String]>,
    pub(super) model: &'a str,
    pub(super) effort: Option<&'a str>,
    pub(super) cwd: &'a Path,
    pub(super) events: Arc<ThreadEventDispatcher>,
    pub(super) alive: Arc<AtomicBool>,
}

pub(super) async fn start(
    args: StartConnection<'_>,
) -> Result<(
    acp::ClientSideConnection,
    tokio::process::Child,
    oneshot::Receiver<()>,
    u32,
)> {
    let StartConnection {
        provider,
        program,
        arguments,
        model,
        effort,
        cwd,
        events,
        alive,
    } = args;
    let command = build_provider_command(program, provider, arguments, model, effort)?;
    let (mut child, process_group) = spawn_provider_process(command, provider, cwd)?;
    let (connection, io_stopped_rx) =
        wire_provider_connection(provider, events, &mut child, alive.clone())?;
    if let Err(error) = initialize(provider, &connection).await {
        terminate_process_group(process_group);
        let _ = child.wait().await;
        return Err(error);
    }
    Ok((connection, child, io_stopped_rx, process_group))
}

/// OpenCode's default `build` agent spawns nested explore/general task subagents. Under Claudex
/// those children inherit max effort on the same model and compete with the parent stream, so
/// Claude Code sees almost no output for minutes while OpenCode burns many internal steps.
/// Runtime config disables task/subagent fan-out only for this ACP child process.
const OPENCODE_ACP_RUNTIME_CONFIG: &str = r#"{"subagent_depth":0,"permission":{"task":"deny"},"agent":{"build":{"permission":{"task":"deny"}}}}"#;

fn build_provider_command(
    program: &OsString,
    provider: AcpProvider,
    arguments: Option<&[String]>,
    model: &str,
    effort: Option<&str>,
) -> Result<Command> {
    let mut command = Command::new(program);
    // Provider ACP children must not re-run Claude Code SessionStart / routing
    // hooks (Herdr stdin, capacity probes). Grok historically used CLAUDEX_GROK_ACP;
    // all ACP providers now also set CLAUDEX_PROVIDER_ACP for a single guard.
    command.env("CLAUDEX_PROVIDER_ACP", "1");
    match provider {
        AcpProvider::Grok => {
            let effort = effort.context("Grok reasoning effort is required at launch")?;
            if !matches!(effort, "low" | "medium" | "high") {
                bail!("Grok reasoning effort must be one of low, medium, or high");
            }
            command.env("CLAUDEX_GROK_ACP", "1");
            command.args([
                "--model",
                model,
                "--reasoning-effort",
                effort,
                "agent",
                "--always-approve",
                "--no-leader",
            ]);
            if let Some(path) = plugin::prepare(program)? {
                command.arg("--plugin-dir").arg(path);
            }
            command.arg("stdio");
        }
        AcpProvider::Copilot => {
            command.args(["--acp", "--stdio", "--model", model]);
        }
        AcpProvider::Configured | AcpProvider::ConfiguredLaunchScoped => {
            let arguments = arguments.context("configured ACP arguments are required")?;
            apply_opencode_acp_runtime_config(&mut command, program);
            for argument in arguments {
                command.arg(substitute_configured_argument(argument, model, effort)?);
            }
        }
    }
    command.env("PATH", path_env::tool_search_path());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.as_std_mut().process_group(0);
    }
    Ok(command)
}

fn substitute_configured_argument(
    argument: &str,
    model: &str,
    effort: Option<&str>,
) -> Result<String> {
    let mut rendered = argument.replace("{model}", model);
    if rendered.contains("{effort}") {
        let effort = effort.context("configured ACP `{effort}` requires launch effort")?;
        rendered = rendered.replace("{effort}", normalize_launch_effort(effort));
    }
    Ok(rendered)
}

/// Map Claudex effort aliases onto values accepted by launch-scoped CLIs such as
/// Cline `--thinking` (`none|low|medium|high|xhigh`).
fn normalize_launch_effort(effort: &str) -> &str {
    match effort {
        "mid" => "medium",
        "max" => "xhigh",
        other => other,
    }
}

#[cfg(test)]
mod argument_tests {
    use super::{normalize_launch_effort, substitute_configured_argument};

    #[test]
    fn substitutes_model_and_thinking_effort() {
        assert_eq!(normalize_launch_effort("max"), "xhigh");
        assert_eq!(normalize_launch_effort("high"), "high");
        let rendered =
            substitute_configured_argument("--thinking", "qwen/qwen3.8-max", Some("high")).unwrap();
        assert_eq!(rendered, "--thinking");
        assert_eq!(
            substitute_configured_argument("{effort}", "m", Some("max")).unwrap(),
            "xhigh"
        );
        assert_eq!(
            substitute_configured_argument("-m", "qwen/qwen3.8-max", None).unwrap(),
            "-m"
        );
        assert_eq!(
            substitute_configured_argument("{model}", "qwen/qwen3.8-max", None).unwrap(),
            "qwen/qwen3.8-max"
        );
        assert!(substitute_configured_argument("{effort}", "m", None).is_err());
    }
}

pub(super) fn is_opencode_program(program: &OsString) -> bool {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "opencode" || name.starts_with("opencode-"))
}

#[cfg(test)]
pub(super) fn opencode_acp_runtime_config() -> &'static str {
    OPENCODE_ACP_RUNTIME_CONFIG
}

fn apply_opencode_acp_runtime_config(command: &mut Command, program: &OsString) {
    if !is_opencode_program(program) {
        return;
    }
    // Leave an explicit user override alone; otherwise inject Claudex's anti-nesting policy.
    if env::var_os("OPENCODE_CONFIG_CONTENT").is_some() {
        return;
    }
    command.env("OPENCODE_CONFIG_CONTENT", OPENCODE_ACP_RUNTIME_CONFIG);
}

fn spawn_provider_process(
    mut command: Command,
    provider: AcpProvider,
    cwd: &Path,
) -> Result<(tokio::process::Child, u32)> {
    let child = command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("start {} ACP server", provider.label()))?;
    let process_group = child
        .id()
        .with_context(|| format!("{} ACP process id is unavailable", provider.label()))?;
    Ok((child, process_group))
}

fn wire_provider_connection(
    provider: AcpProvider,
    events: Arc<ThreadEventDispatcher>,
    child: &mut tokio::process::Child,
    alive: Arc<AtomicBool>,
) -> Result<(acp::ClientSideConnection, oneshot::Receiver<()>)> {
    let outgoing = child
        .stdin
        .take()
        .with_context(|| format!("{} ACP stdin is unavailable", provider.label()))?
        .compat_write();
    let incoming = child
        .stdout
        .take()
        .with_context(|| format!("{} ACP stdout is unavailable", provider.label()))?
        .compat();
    let client = AcpClient::new(events);
    let (connection, handle_io) =
        acp::ClientSideConnection::new(client, outgoing, incoming, |future| {
            tokio::task::spawn_local(future);
        });
    let (io_stopped, io_stopped_rx) = oneshot::channel();
    let provider_label = provider.label();
    tokio::task::spawn_local(async move {
        if let Err(error) = handle_io.await {
            tracing::error!(
                ?error,
                provider = provider_label,
                "ACP I/O stopped (provider likely exited; recycle the route)"
            );
        }
        mark_io_stopped(&alive, io_stopped);
    });
    Ok((connection, io_stopped_rx))
}

fn mark_io_stopped(alive: &AtomicBool, io_stopped: oneshot::Sender<()>) {
    alive.store(false, Ordering::Relaxed);
    let _ = io_stopped.send(());
}

pub(super) fn terminate_process_group(process_group: u32) {
    let _status = StdCommand::new("kill")
        .args(["-KILL", &format!("-{process_group}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

async fn initialize(provider: AcpProvider, connection: &acp::ClientSideConnection) -> Result<()> {
    let response = connection
        .initialize(
            acp::InitializeRequest::new(acp::ProtocolVersion::V1)
                .client_info(acp::Implementation::new(
                    "claudex-agent-adapter",
                    env!("CARGO_PKG_VERSION"),
                ))
                .meta(
                    json!({
                        "startupHints": {
                            "nonInteractive": true,
                            "skipGitStatus": true,
                            "skipProjectLayout": true
                        },
                        "clientType":"claudex-agent-adapter"
                    })
                    .as_object()
                    .cloned(),
                ),
        )
        .await
        .map_err(|error| anyhow!("{} ACP initialize failed: {error:?}", provider.label()))?;
    if response.protocol_version != acp::ProtocolVersion::V1 {
        bail!(
            "{} ACP selected unsupported protocol version",
            provider.label()
        )
    }
    let preferred = response
        .meta
        .as_ref()
        .and_then(|meta| meta.get("defaultAuthMethodId"))
        .and_then(Value::as_str);
    let method = preferred
        .and_then(|id| {
            response
                .auth_methods
                .iter()
                .find(|method| method.id().0.as_ref() == id)
        })
        .or_else(|| response.auth_methods.first());
    if let Some(method) = method {
        connection
            .authenticate(
                acp::AuthenticateRequest::new(method.id().clone())
                    .meta(json!({"headless":true}).as_object().cloned()),
            )
            .await
            .map_err(|error| {
                anyhow!("{} ACP authentication failed: {error:?}", provider.label())
            })?;
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "connection_tests.rs"]
mod tests;
