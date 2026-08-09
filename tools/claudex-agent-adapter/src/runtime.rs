use std::{collections::VecDeque, ffi::OsString, path::PathBuf, sync::Arc, time::Duration};

use crate::{
    agent_backend::{AgentBackend, BackendKind, BackendRoute},
    anthropic::{Bridge, DEFAULT_MAX_PROCESSES, DEFAULT_TIMEOUT_MINUTES},
    http_router,
    launcher::{self, AdapterOptions},
    provider_config::{self, WorkerRoute},
};
use anyhow::{Context, Result, bail};

mod hard_timeout;
mod shutdown;

#[derive(Debug)]
enum RuntimeCommand {
    BuildId,
    Ensure(AdapterOptions),
    HotSwap(AdapterOptions),
    Launch(AdapterOptions, Vec<OsString>, bool),
    McpClaudexLaunch,
    Serve(AdapterOptions),
}

#[derive(Debug)]
struct ParsedOptions {
    adapter: AdapterOptions,
    inherit_claude_model: bool,
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
        RuntimeCommand::HotSwap(options) => {
            println!("{}", launcher::hot_swap(options).await?);
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
            let options = parse_options(&mut arguments)?;
            reject_inherit_model(&options, "hot-swap")?;
            reject_remaining(&arguments)?;
            Ok(RuntimeCommand::HotSwap(options.adapter))
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

// Keep the option table in one place so each supported CLI flag has a single,
// auditable parsing branch; the build gate still limits this file to 400 lines.
#[allow(clippy::too_many_lines)]
fn parse_options(arguments: &mut VecDeque<OsString>) -> Result<ParsedOptions> {
    let mut routes = Vec::new();
    let mut worker_routes = Vec::new();
    let mut search_worker_routes = Vec::new();
    let mut model = None;
    let mut provider_config = None;
    let mut inherit_claude_model = false;
    let mut listen = "127.0.0.1:8318".parse().expect("default listener");
    let mut max_processes = DEFAULT_MAX_PROCESSES;
    let mut timeout_minutes = DEFAULT_TIMEOUT_MINUTES;
    let mut hard_timeout_cli = None;
    while let Some(option) = arguments
        .front()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
    {
        match option.as_str() {
            "--backend-route" => {
                routes.push(option_value(arguments, "--backend-route")?.parse()?);
            }
            "--backend-route-json" => {
                let value = option_value(arguments, "--backend-route-json")?;
                routes.push(serde_json::from_str(&value).context("invalid backend route JSON")?);
            }
            "--worker-route-json" => {
                let value = option_value(arguments, "--worker-route-json")?;
                worker_routes.push(
                    serde_json::from_str::<WorkerRoute>(&value)
                        .context("invalid worker route JSON")?,
                );
            }
            "--search-worker-route-json" => {
                let value = option_value(arguments, "--search-worker-route-json")?;
                search_worker_routes.push(
                    serde_json::from_str::<WorkerRoute>(&value)
                        .context("invalid search worker route JSON")?,
                );
            }
            "--provider-config" => {
                provider_config =
                    Some(PathBuf::from(option_value(arguments, "--provider-config")?));
            }
            "--model" => model = Some(option_value(arguments, "--model")?),
            "--inherit-claude-model" => {
                arguments.pop_front();
                inherit_claude_model = true;
            }
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
            "--subagent-hard-timeout-seconds" => {
                parse_hard_timeout(arguments, &option, &mut hard_timeout_cli)?;
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
    let hard_timeout = hard_timeout::resolve(hard_timeout_cli)?;
    let configured = provider_config
        .as_deref()
        .map(provider_config::load)
        .transpose()?;
    let has_provider_config = configured.is_some();
    let mut model_catalog = configured
        .as_ref()
        .map(|configured| configured.model_catalog.clone())
        .unwrap_or_default();

    if let Some(configured) = &configured {
        routes.splice(0..0, configured.routes.clone());
    }
    if model.as_deref().is_some_and(str::is_empty) {
        bail!("--model must not be empty");
    }
    let model = model.unwrap_or_default();
    if model.is_empty() && !has_provider_config && routes.is_empty() {
        bail!("--model or --provider-config is required");
    }
    if routes.is_empty() {
        routes.push(BackendRoute::new(&model, BackendKind::CodexAppServer));
    }
    if model_catalog == provider_config::ModelCatalog::default() {
        model_catalog = provider_config::ModelCatalog::from_routes(&routes);
    }
    if !worker_routes.is_empty() {
        model_catalog.set_worker_routes(worker_routes)?;
    }
    if !search_worker_routes.is_empty() {
        model_catalog.set_search_worker_routes(search_worker_routes)?;
    }
    validate_routes(&routes)?;
    Ok(ParsedOptions {
        adapter: AdapterOptions {
            routes,
            model,
            listen,
            subscription_max_processes: max_processes,
            subscription_timeout_minutes: timeout_minutes,
            subagent_hard_timeout_seconds: hard_timeout,
            model_catalog,
        },
        inherit_claude_model,
    })
}

fn parse_hard_timeout(
    arguments: &mut VecDeque<OsString>,
    option: &str,
    hard_timeout: &mut Option<std::num::NonZeroU64>,
) -> Result<()> {
    if hard_timeout.is_some() {
        bail!("--subagent-hard-timeout-seconds must not be repeated");
    }
    let seconds: u64 = positive_number(arguments, option)?;
    *hard_timeout = std::num::NonZeroU64::new(seconds);
    Ok(())
}

fn reject_inherit_model(options: &ParsedOptions, command: &str) -> Result<()> {
    if options.inherit_claude_model {
        bail!("--inherit-claude-model is valid only for launch, not {command}");
    }
    Ok(())
}

fn validate_routes(routes: &[BackendRoute]) -> Result<()> {
    if routes.iter().any(|route| {
        route
            .max_concurrency
            .is_some_and(|limit| limit == 0 || limit > crate::grok_acp::MAX_MODEL_CONCURRENCY)
    }) {
        bail!("backend route maxConcurrency is out of range");
    }
    let unique = routes
        .iter()
        .map(|route| route.model.as_str())
        .collect::<std::collections::HashSet<_>>();
    if unique.len() != routes.len() {
        bail!("--backend-route models must be unique");
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
    let result = shutdown::serve(
        listener,
        http_router(Arc::clone(&bridge), options.model, auth_token),
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
include!("runtime_tests.rs");
