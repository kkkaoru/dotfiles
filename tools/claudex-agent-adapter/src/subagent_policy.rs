use std::{collections::BTreeSet, ffi::OsStr, process::Command, time::Duration};

use anyhow::{Context, Result, bail};
use axum::http::HeaderMap;

pub(crate) const ENV_NAME: &str = "CLAUDEX_DISABLED_SUBAGENT_MODELS";
pub(crate) const CONFIG_ENV_NAME: &str = "CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG";
pub(crate) const RESOLVED_ENV_NAME: &str = "CLAUDEX_RESOLVED_DISABLED_SUBAGENT_MODELS";
pub(crate) const HEADER_NAME: &str = "x-claudex-disabled-subagent-models";
pub(super) const CONFIG_VERSION: u64 = 1;
pub(super) const CONFIG_DIR_RELATIVE: &str = ".config/claudex";
pub(super) const CONFIG_RELATIVE_PATH: &str = ".config/claudex/disabled-subagent-models.json";
pub(super) const LOCAL_CONFIG_NAME: &str = "disabled-subagent-models.local.json";
pub(crate) const DENY_ALL_SENTINEL: &str = "__claudex_deny_all_subagent_models__";
pub(super) const LOAD_ATTEMPTS: usize = 3;
#[cfg(not(test))]
pub(super) const LOAD_RETRY: Duration = Duration::from_millis(10);
#[cfg(test)]
pub(super) const LOAD_RETRY: Duration = Duration::ZERO;

#[path = "subagent_policy_load.rs"]
mod load;
pub(crate) use load::denylist_load_warning;
#[cfg(test)]
use load::{
    clear_denylist_cache, clear_memory_keep_disk_last_good, load_config_from_reader, short_hostname,
};
use load::{config_path, load_config, surface_denylist_error};

pub(crate) fn active_header() -> Option<String> {
    match configured_header() {
        Ok(header) => header,
        Err(error) => Some(
            surface_denylist_error(error)
                .into_iter()
                .collect::<Vec<_>>()
                .join(","),
        ),
    }
}

fn configured_header() -> Result<Option<String>> {
    let path = config_path(
        std::env::var_os(CONFIG_ENV_NAME).as_deref(),
        std::env::var_os("HOME").as_deref(),
    )?;
    merged_header(
        &load_config(path.as_deref()),
        std::env::var_os(ENV_NAME).as_deref(),
    )
}

/// Resolve the current machine policy for every incoming request. The launcher
/// snapshots this value into Claude's environment, but an already-running
/// Claude process may not have that snapshot; the HTTP boundary must still
/// enforce the current denylist.
pub(crate) fn active_models() -> BTreeSet<String> {
    match configured_header().and_then(|header| {
        header
            .map(|header| parse(&header))
            .transpose()
            .map(Option::unwrap_or_default)
    }) {
        Ok(models) => models,
        Err(error) => surface_denylist_error(error),
    }
}

pub(crate) fn model_is_disabled(disabled: &BTreeSet<String>, model: &str) -> bool {
    disabled.contains(DENY_ALL_SENTINEL) || disabled.contains(model)
}

pub(crate) fn header_value(header: &Option<String>) -> &str {
    header.as_deref().unwrap_or_default()
}

pub(crate) fn apply_snapshot(command: &mut Command, header: &Option<String>) {
    command.env(RESOLVED_ENV_NAME, header_value(header));
}

pub(crate) fn merged_header(
    configured: &BTreeSet<String>,
    value: Option<&OsStr>,
) -> Result<Option<String>> {
    let mut models = configured.clone();
    if let Some(value) = value {
        models.extend(
            value
                .to_str()
                .context("CLAUDEX_DISABLED_SUBAGENT_MODELS must be valid UTF-8")
                .and_then(parse)?,
        );
    }
    Ok((!models.is_empty()).then(|| models.into_iter().collect::<Vec<_>>().join(",")))
}

pub(crate) fn request_models(headers: &HeaderMap) -> Result<BTreeSet<String>> {
    headers
        .get(HEADER_NAME)
        .map(|value| {
            value
                .to_str()
                .context("disabled SubAgent model header must be visible ASCII")
                .and_then(parse)
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(super) fn parse(value: &str) -> Result<BTreeSet<String>> {
    value
        .split(',')
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(|model| {
            if valid_model_id(model) {
                return Ok(model.to_owned());
            }
            bail!("{ENV_NAME} contains an invalid model ID")
        })
        .collect()
}

pub(crate) fn valid_model_id(model: &str) -> bool {
    !model.is_empty() && model.is_ascii() && model.bytes().all(|byte| byte.is_ascii_graphic())
}

#[cfg(test)]
include!("subagent_policy_tests.rs");
