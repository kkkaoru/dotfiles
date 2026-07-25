use std::{
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
use crate::app_server::events::ThreadEventDispatcher;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AcpProvider {
    Grok,
    Copilot,
    Configured,
}

impl AcpProvider {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Grok => "Grok",
            Self::Copilot => "Copilot",
            Self::Configured => "Configured",
        }
    }

    pub(super) const fn driver_name(self) -> &'static str {
        match self {
            Self::Grok => "claudex-grok-acp",
            Self::Copilot => "claudex-copilot-acp",
            Self::Configured => "claudex-configured-acp",
        }
    }
}

pub(super) async fn start(
    provider: AcpProvider,
    program: &OsString,
    arguments: Option<&[String]>,
    model: &str,
    cwd: &Path,
    events: Arc<ThreadEventDispatcher>,
    alive: Arc<AtomicBool>,
) -> Result<(
    acp::ClientSideConnection,
    tokio::process::Child,
    oneshot::Receiver<()>,
    u32,
)> {
    let mut command = Command::new(program);
    match provider {
        AcpProvider::Grok => {
            // Grok runs Claude-compatible project hooks but does not close their stdin. Mark the
            // child so the SessionStart hook can skip its blocking Claude-only state report.
            command.env("CLAUDEX_GROK_ACP", "1");
            command.args(["--model", model, "agent", "--always-approve"]);
            if let Some(path) = plugin::prepare(program)? {
                command.arg("--plugin-dir").arg(path);
            }
            command.arg("stdio");
        }
        AcpProvider::Copilot => {
            command.args(["--acp", "--stdio", "--model", model]);
        }
        AcpProvider::Configured => {
            let arguments = arguments.context("configured ACP arguments are required")?;
            command.args(
                arguments
                    .iter()
                    .map(|argument| argument.replace("{model}", model)),
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.as_std_mut().process_group(0);
    }
    let mut child = command
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
    tokio::task::spawn_local(async move {
        if let Err(error) = handle_io.await {
            tracing::error!(?error, provider = provider.label(), "ACP I/O stopped");
        }
        // Mark the provider dead before failed RPCs wake their callers. Otherwise session/new can
        // observe a closed connection while the driver still appears alive and skip route recovery.
        alive.store(false, Ordering::Relaxed);
        let _ = io_stopped.send(());
    });
    if let Err(error) = initialize(provider, &connection).await {
        terminate_process_group(process_group);
        return Err(error);
    }
    Ok((connection, child, io_stopped_rx, process_group))
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
