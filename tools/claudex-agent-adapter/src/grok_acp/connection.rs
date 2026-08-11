use std::{
    ffi::OsString,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

use agent_client_protocol as acp;
use anyhow::Result;
use tokio::sync::oneshot;

use crate::app_server::events::ThreadEventDispatcher;

mod handshake;
#[cfg(test)]
use handshake::mark_io_stopped;
use handshake::{initialize, wire_provider_connection};

#[path = "connection_command.rs"]
mod command;
#[cfg(test)]
pub(super) use command::is_opencode_program;
pub(super) use command::terminate_process_group;
#[cfg(test)]
pub(super) use command::{
    OPENCODE_ACP_RUNTIME_CONFIG, apply_opencode_acp_runtime_config, normalize_launch_effort,
    opencode_acp_runtime_config, substitute_configured_argument,
};
use command::{build_provider_command, spawn_provider_process};

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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "connection_argument_tests.rs"]
mod argument_tests;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "connection_tests.rs"]
mod tests;
