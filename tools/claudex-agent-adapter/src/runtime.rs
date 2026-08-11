use std::{collections::VecDeque, ffi::OsString, path::PathBuf, sync::Arc, time::Duration};

use crate::{
    agent_backend::AgentBackend,
    anthropic::Bridge,
    http_api::http_router_with_handover,
    launcher::{self, AdapterOptions},
};
use anyhow::{Context, Result, bail};

mod hard_timeout;
mod parse_options;
mod shutdown;
use parse_options::{ParsedOptions, parse_options};

#[derive(Debug)]
enum RuntimeCommand {
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

fn parse_command(mut arguments: VecDeque<OsString>) -> Result<RuntimeCommand> {
    let command = utf8(arguments.pop_front(), "command")?;
    match command.as_str() {
        "build-id" => {
            reject_remaining(&arguments)?;
            Ok(RuntimeCommand::BuildId)
        }
        "ensure" => {
            let options = parse_options(&mut arguments)?;
            reject_inherit_model(&options, "ensure")?;
            reject_remaining(&arguments)?;
            Ok(RuntimeCommand::Ensure(options.adapter))
        }
        "hot-swap" => {
            let wait_idle = take_flag(&mut arguments, "--wait-idle");
            let options = parse_options(&mut arguments)?;
            reject_inherit_model(&options, "hot-swap")?;
            reject_remaining(&arguments)?;
            Ok(RuntimeCommand::HotSwap(options.adapter, wait_idle))
        }
        "launch" => {
            let options = parse_options(&mut arguments)?;
            consume_separator(&mut arguments)?;
            let inherit_claude_model =
                options.inherit_claude_model || options.adapter.model.is_empty();
            Ok(RuntimeCommand::Launch(
                options.adapter,
                arguments.into(),
                inherit_claude_model,
            ))
        }
        "mcp-claudex-launch" => {
            reject_remaining(&arguments)?;
            Ok(RuntimeCommand::McpClaudexLaunch)
        }
        "serve" => {
            let options = parse_options(&mut arguments)?;
            reject_inherit_model(&options, "serve")?;
            reject_remaining(&arguments)?;
            Ok(RuntimeCommand::Serve(options.adapter))
        }
        _ => bail!(
            "unknown command `{command}`; expected build-id, ensure, hot-swap, launch, mcp-claudex-launch, or serve"
        ),
    }
}

fn reject_inherit_model(options: &ParsedOptions, command: &str) -> Result<()> {
    if options.inherit_claude_model {
        bail!("--inherit-claude-model is valid only for launch, not {command}");
    }
    Ok(())
}

fn consume_separator(arguments: &mut VecDeque<OsString>) -> Result<()> {
    if arguments.front().and_then(|value| value.to_str()) == Some("--") {
        arguments.pop_front();
        return Ok(());
    }
    bail!("launch requires `--` before Claude Code arguments")
}

fn reject_remaining(arguments: &VecDeque<OsString>) -> Result<()> {
    if arguments.is_empty() {
        return Ok(());
    }
    bail!("unexpected arguments after adapter options")
}

fn take_flag(arguments: &mut VecDeque<OsString>, flag: &str) -> bool {
    arguments
        .iter()
        .position(|value| value == flag)
        .map(|index| {
            arguments.remove(index);
            true
        })
        .unwrap_or(false)
}

fn utf8(value: Option<OsString>, name: &str) -> Result<String> {
    value
        .with_context(|| format!("{name} is required"))?
        .into_string()
        .map_err(|_| anyhow::anyhow!("{name} must be valid UTF-8"))
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
