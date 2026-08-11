use std::{collections::VecDeque, ffi::OsString, net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result, bail};

use crate::{
    agent_backend::{BackendKind, BackendRoute},
    anthropic::{DEFAULT_MAX_PROCESSES, DEFAULT_TIMEOUT_MINUTES},
    launcher::AdapterOptions,
    provider_config::{self, WorkerRoute},
};

use super::hard_timeout;

#[derive(Debug)]
pub(super) struct ParsedOptions {
    pub(super) adapter: AdapterOptions,
    pub(super) inherit_claude_model: bool,
}

#[derive(Debug)]
struct OptionsDraft {
    routes: Vec<BackendRoute>,
    worker_routes: Vec<WorkerRoute>,
    search_worker_routes: Vec<WorkerRoute>,
    selectable_models: Vec<String>,
    model: Option<String>,
    provider_config: Option<PathBuf>,
    inherit_claude_model: bool,
    listen: SocketAddr,
    max_processes: usize,
    timeout_minutes: u64,
    hard_timeout_cli: Option<std::num::NonZeroU64>,
}

impl Default for OptionsDraft {
    fn default() -> Self {
        Self {
            routes: Vec::new(),
            worker_routes: Vec::new(),
            search_worker_routes: Vec::new(),
            selectable_models: Vec::new(),
            model: None,
            provider_config: None,
            inherit_claude_model: false,
            listen: "127.0.0.1:8318".parse().expect("default listener"),
            max_processes: DEFAULT_MAX_PROCESSES,
            timeout_minutes: DEFAULT_TIMEOUT_MINUTES,
            hard_timeout_cli: None,
        }
    }
}

// Keep CLI flags in one table so each parsing branch stays auditable.
pub(super) fn parse_options(arguments: &mut VecDeque<OsString>) -> Result<ParsedOptions> {
    let mut draft = OptionsDraft::default();
    while let Some(option) = arguments
        .front()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
    {
        if option == "--" {
            break;
        }
        apply_option(arguments, &option, &mut draft)?;
    }
    if arguments
        .front()
        .is_some_and(|value| value.to_str().is_none())
    {
        bail!("adapter options must be valid UTF-8");
    }
    assemble_options(draft)
}

fn apply_option(
    arguments: &mut VecDeque<OsString>,
    option: &str,
    draft: &mut OptionsDraft,
) -> Result<()> {
    match option {
        "--backend-route" => {
            draft
                .routes
                .push(option_value(arguments, "--backend-route")?.parse()?);
        }
        "--backend-route-json" => {
            let value = option_value(arguments, "--backend-route-json")?;
            draft
                .routes
                .push(serde_json::from_str(&value).context("invalid backend route JSON")?);
        }
        "--worker-route-json" => {
            let value = option_value(arguments, "--worker-route-json")?;
            draft.worker_routes.push(
                serde_json::from_str::<WorkerRoute>(&value).context("invalid worker route JSON")?,
            );
        }
        "--search-worker-route-json" => {
            let value = option_value(arguments, "--search-worker-route-json")?;
            draft.search_worker_routes.push(
                serde_json::from_str::<WorkerRoute>(&value)
                    .context("invalid search worker route JSON")?,
            );
        }
        "--selectable-model" => {
            draft
                .selectable_models
                .push(option_value(arguments, "--selectable-model")?);
        }
        "--provider-config" => {
            draft.provider_config =
                Some(PathBuf::from(option_value(arguments, "--provider-config")?));
        }
        "--model" => draft.model = Some(option_value(arguments, "--model")?),
        "--inherit-claude-model" => {
            arguments.pop_front();
            draft.inherit_claude_model = true;
        }
        "--listen" => {
            draft.listen = option_value(arguments, "--listen")?
                .parse()
                .context("invalid --listen address")?;
        }
        "--subscription-max-processes" | "--subscription-timeout-minutes" => {
            apply_limit_option(arguments, option, draft)?;
        }
        "--subagent-hard-timeout-seconds" => {
            parse_hard_timeout(arguments, option, &mut draft.hard_timeout_cli)?;
        }
        _ => bail!("unknown adapter option `{option}`"),
    }
    Ok(())
}

fn apply_limit_option(
    arguments: &mut VecDeque<OsString>,
    option: &str,
    draft: &mut OptionsDraft,
) -> Result<()> {
    match option {
        "--subscription-max-processes" => {
            draft.max_processes = positive_number(arguments, option)?;
        }
        "--subscription-timeout-minutes" => {
            draft.timeout_minutes = positive_number(arguments, option)?;
        }
        _ => unreachable!("limit option filter"),
    }
    Ok(())
}

fn assemble_options(mut draft: OptionsDraft) -> Result<ParsedOptions> {
    validate_limits(draft.max_processes, draft.timeout_minutes)?;
    let hard_timeout = hard_timeout::resolve(draft.hard_timeout_cli)?;
    let configured = draft
        .provider_config
        .as_deref()
        .map(provider_config::load)
        .transpose()?;
    let has_provider_config = configured.is_some();
    let mut model_catalog = configured
        .as_ref()
        .map(|configured| configured.model_catalog.clone())
        .unwrap_or_default();
    if let Some(configured) = &configured {
        draft.routes.splice(0..0, configured.routes.clone());
    }
    if draft.model.as_deref().is_some_and(str::is_empty) {
        bail!("--model must not be empty");
    }
    let model = draft.model.unwrap_or_default();
    if model.is_empty() && !has_provider_config && draft.routes.is_empty() {
        bail!("--model or --provider-config is required");
    }
    if draft.routes.is_empty() {
        draft
            .routes
            .push(BackendRoute::new(&model, BackendKind::CodexAppServer));
    }
    if model_catalog == provider_config::ModelCatalog::default() {
        model_catalog = provider_config::ModelCatalog::from_routes(&draft.routes);
    }
    if !draft.worker_routes.is_empty() {
        model_catalog.set_worker_routes(draft.worker_routes)?;
    }
    if !draft.search_worker_routes.is_empty() {
        model_catalog.set_search_worker_routes(draft.search_worker_routes)?;
    }
    if !draft.selectable_models.is_empty() {
        model_catalog.set_selectable_models(draft.selectable_models);
    }
    validate_routes(&draft.routes)?;
    Ok(ParsedOptions {
        adapter: AdapterOptions {
            routes: draft.routes,
            model,
            listen: draft.listen,
            subscription_max_processes: draft.max_processes,
            subscription_timeout_minutes: draft.timeout_minutes,
            subagent_hard_timeout_seconds: hard_timeout,
            model_catalog,
        },
        inherit_claude_model: draft.inherit_claude_model,
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

fn utf8(value: Option<OsString>, name: &str) -> Result<String> {
    value
        .with_context(|| format!("{name} is required"))?
        .into_string()
        .map_err(|_| anyhow::anyhow!("{name} must be valid UTF-8"))
}
