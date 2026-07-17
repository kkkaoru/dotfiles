use std::{collections::VecDeque, ffi::OsString, sync::Arc};

use anyhow::{Context, Result, bail};
use tracing_subscriber::EnvFilter;

use crate::{
    agent_backend::{AgentBackend, BackendKind, BackendRoute},
    anthropic::{Bridge, DEFAULT_MAX_PROCESSES, DEFAULT_TIMEOUT_MINUTES},
    http_router,
    launcher::{self, AdapterOptions},
};

#[derive(Debug)]
enum RuntimeCommand {
    BuildId,
    Ensure(AdapterOptions),
    Launch(AdapterOptions, Vec<OsString>),
    Serve(AdapterOptions),
}

pub async fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<i32> {
    let code = match parse_command(arguments.into_iter().skip(1).collect())? {
        RuntimeCommand::BuildId => {
            println!("{}", env!("CLAUDEX_BUILD_ID"));
            0
        }
        RuntimeCommand::Ensure(options) => {
            println!("{}", launcher::ensure_running(options).await?);
            0
        }
        RuntimeCommand::Launch(options, arguments) => {
            launcher::run_claude(options, arguments).await?
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
            reject_remaining(&arguments)?;
            Ok(RuntimeCommand::Ensure(options))
        }
        "launch" => {
            let options = parse_options(&mut arguments)?;
            consume_separator(&mut arguments)?;
            Ok(RuntimeCommand::Launch(options, arguments.into()))
        }
        "serve" => {
            let options = parse_options(&mut arguments)?;
            reject_remaining(&arguments)?;
            Ok(RuntimeCommand::Serve(options))
        }
        _ => bail!("unknown command `{command}`; expected build-id, ensure, launch, or serve"),
    }
}

fn parse_options(arguments: &mut VecDeque<OsString>) -> Result<AdapterOptions> {
    let mut routes = Vec::new();
    let mut model = None;
    let mut listen = "127.0.0.1:8318".parse().expect("default listener");
    let mut max_processes = DEFAULT_MAX_PROCESSES;
    let mut timeout_minutes = DEFAULT_TIMEOUT_MINUTES;
    while let Some(option) = arguments
        .front()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
    {
        match option.as_str() {
            "--backend-route" => {
                routes.push(option_value(arguments, "--backend-route")?.parse()?);
            }
            "--model" => model = Some(option_value(arguments, "--model")?),
            "--listen" => {
                listen = option_value(arguments, "--listen")?
                    .parse()
                    .context("invalid --listen address")?;
            }
            "--subscription-max-processes" => {
                max_processes = positive_number(arguments, &option)?;
            }
            "--subscription-timeout-minutes" => {
                timeout_minutes = positive_number(arguments, &option)?;
            }
            "--" => break,
            _ => bail!("unknown adapter option `{option}`"),
        }
    }
    if arguments
        .front()
        .is_some_and(|value| value.to_str().is_none())
    {
        bail!("adapter options must be valid UTF-8");
    }
    validate_limits(max_processes, timeout_minutes)?;
    let model = model.context("--model is required")?;
    if routes.is_empty() {
        routes.push(BackendRoute {
            model: model.clone(),
            backend: BackendKind::CodexAppServer,
        });
    }
    validate_routes(&routes, &model)?;
    Ok(AdapterOptions {
        routes,
        model,
        listen,
        subscription_max_processes: max_processes,
        subscription_timeout_minutes: timeout_minutes,
    })
}

fn validate_routes(routes: &[BackendRoute], model: &str) -> Result<()> {
    let unique = routes
        .iter()
        .map(|route| route.model.as_str())
        .collect::<std::collections::HashSet<_>>();
    if unique.len() != routes.len() {
        bail!("--backend-route models must be unique");
    }
    if !unique.contains(model) {
        bail!("the main --model must have a --backend-route");
    }
    Ok(())
}

fn validate_limits(max_processes: usize, timeout_minutes: u64) -> Result<()> {
    if max_processes > tokio::sync::Semaphore::MAX_PERMITS {
        bail!("--subscription-max-processes is out of range");
    }
    if timeout_minutes.checked_mul(60).is_none() {
        bail!("--subscription-timeout-minutes is out of range");
    }
    Ok(())
}

fn option_value(arguments: &mut VecDeque<OsString>, option: &str) -> Result<String> {
    arguments.pop_front();
    utf8(
        arguments.pop_front(),
        &format!("value for adapter option {option}"),
    )
}

fn positive_number<T>(arguments: &mut VecDeque<OsString>, option: &str) -> Result<T>
where
    T: std::str::FromStr + PartialOrd + From<u8>,
{
    let value = option_value(arguments, option)?;
    value
        .parse::<T>()
        .ok()
        .filter(|number| *number > T::from(0))
        .with_context(|| format!("{option} must be a positive integer"))
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

fn utf8(value: Option<OsString>, name: &str) -> Result<String> {
    value
        .with_context(|| format!("{name} is required"))?
        .into_string()
        .map_err(|_| anyhow::anyhow!("{name} must be valid UTF-8"))
}

pub async fn serve(options: AdapterOptions) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init()
        .ok();
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
    let bridge = Arc::new(Bridge::new_with_backend_limits(
        backend,
        options.model.clone(),
        options.subscription_max_processes,
        options.subscription_timeout_minutes,
    )?);
    tracing::info!(listen = %options.listen, routes = ?options.routes, model = %options.model, "claudex agent adapter is ready");
    axum::serve(listener, http_router(bridge, options.model, auth_token))
        .await
        .map_err(Into::into)
}

fn configured_token() -> Option<String> {
    nonempty_token(std::env::var("ANTHROPIC_AUTH_TOKEN").ok())
}

fn nonempty_token(token: Option<String>) -> Option<String> {
    token.filter(|token| !token.is_empty())
}

#[cfg(test)]
include!("runtime_tests.rs");
