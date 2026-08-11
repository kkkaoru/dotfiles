use std::{collections::VecDeque, ffi::OsString, net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result, bail};

use crate::{
    agent_backend::BackendRoute,
    anthropic::{DEFAULT_MAX_PROCESSES, DEFAULT_TIMEOUT_MINUTES},
    launcher::AdapterOptions,
    provider_config::WorkerRoute,
};

use super::hard_timeout;

#[path = "parse_options_assemble.rs"]
mod assemble;
#[path = "parse_options_limits.rs"]
mod limits;
use assemble::assemble_options;
use limits::{apply_limit_option, option_value, parse_hard_timeout};

#[derive(Debug)]
pub(super) struct ParsedOptions {
    pub(super) adapter: AdapterOptions,
    pub(super) inherit_claude_model: bool,
}

#[derive(Debug)]
pub(super) struct OptionsDraft {
    pub(super) routes: Vec<BackendRoute>,
    pub(super) worker_routes: Vec<WorkerRoute>,
    pub(super) search_worker_routes: Vec<WorkerRoute>,
    pub(super) selectable_models: Vec<String>,
    pub(super) model: Option<String>,
    pub(super) provider_config: Option<PathBuf>,
    pub(super) inherit_claude_model: bool,
    pub(super) listen: SocketAddr,
    pub(super) max_processes: usize,
    pub(super) timeout_minutes: u64,
    pub(super) hard_timeout_cli: Option<std::num::NonZeroU64>,
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

