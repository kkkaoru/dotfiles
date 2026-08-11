use std::{ffi::OsString, path::PathBuf, sync::Arc, time::Duration};

use crate::{
    agent_backend::AgentBackend,
    anthropic::Bridge,
    http_api::http_router_with_handover,
    launcher::{self, AdapterOptions},
};
use anyhow::{Result, bail};

mod command_helpers;
mod hard_timeout;
mod parse_options;
mod shutdown;
#[path = "runtime_parse_command.rs"]
mod parse_command;
use parse_command::parse_command;
#[cfg(test)]
#[allow(unused_imports)]
use command_helpers::utf8;

#[derive(Debug)]
pub(super) enum RuntimeCommand {
    BuildId,
    Ensure(AdapterOptions),
    HotSwap(AdapterOptions, bool),
    Launch(AdapterOptions, Vec<OsString>, bool),
    McpClaudexLaunch,
    Serve(AdapterOptions),
}

pub async fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<i32> {
    let code = match parse_command(arguments.into_iter().skip(1).collect())? {
        RuntimeCommand::BuildId => {
            println!("{}", env!("CLAUDEX_BUILD_ID"));
            0
        }
        RuntimeCommand::Ensure(options) => {
            println!("{}", launcher::ensure_running_cli(options).await?);
            0
        }
        RuntimeCommand::HotSwap(options, wait_idle) => {
            println!("{}", launcher::hot_swap_cli(options, wait_idle).await?);
            0
        }
        RuntimeCommand::Launch(options, arguments, inherit_claude_model) => {
            launcher::run_claude(options, arguments, inherit_claude_model).await?
        }
        RuntimeCommand::McpClaudexLaunch => {
            crate::launch_mcp::run_stdio()?;
            0
        }
        RuntimeCommand::Serve(options) => {
            serve(options).await?;
            0
        }
    };
    Ok(code)
}



pub async fn serve(options: AdapterOptions) -> Result<()> {
    crate::logging::init();
    let auth_token = configured_token();
    if !options.listen.ip().is_loopback() & auth_token.is_none() {
        bail!("ANTHROPIC_AUTH_TOKEN is required for a non-loopback listener");
    }
    let backend = AgentBackend::spawn_routes(&options.routes);
    let listener = tokio::net::TcpListener::bind(options.listen).await?;
    serve_on_listener(options, auth_token, backend, listener).await
}

async fn serve_on_listener(
    options: AdapterOptions,
    auth_token: Option<String>,
    backend: Arc<AgentBackend>,
    listener: tokio::net::TcpListener,
) -> Result<()> {
    let bridge = Arc::new(
        Bridge::new_with_backend_limits(
            Arc::clone(&backend),
            options.model.clone(),
            options.subscription_max_processes,
            options.subscription_timeout_minutes,
        )?
        .with_subagent_hard_timeout(
            options
                .subagent_hard_timeout_seconds
                .map(|seconds| Duration::from_secs(seconds.get())),
        )
        .with_persisted_agent_intents()
        .with_model_catalog(options.model_catalog.clone()),
    );
    tracing::info!(listen = %options.listen, routes = ?options.routes, model = %options.model, "claudex agent adapter is ready");
    let cache = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cache/claudex");
    let (handover, rx) =
        crate::listen_handover::ListenHandover::from_runtime_bind(options.listen, cache);
    let handover_listener = crate::listen_handover::HandoverListener::new(listener, &handover, rx);
    let result = shutdown::serve(
        handover_listener,
        http_router_with_handover(
            Arc::clone(&bridge),
            options.model,
            auth_token,
            Some(handover),
        ),
    )
    .await;
    backend.shutdown().await;
    result
}

fn configured_token() -> Option<String> {
    nonempty_token(std::env::var("ANTHROPIC_AUTH_TOKEN").ok())
}
fn nonempty_token(token: Option<String>) -> Option<String> {
    token.filter(|token| !token.is_empty())
}
#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
