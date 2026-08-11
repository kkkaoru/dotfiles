use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use axum::http::HeaderMap;
use serde::Deserialize;

pub(crate) const ENV_NAME: &str = "CLAUDEX_DISABLED_SUBAGENT_MODELS";
pub(crate) const CONFIG_ENV_NAME: &str = "CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG";
pub(crate) const RESOLVED_ENV_NAME: &str = "CLAUDEX_RESOLVED_DISABLED_SUBAGENT_MODELS";
pub(crate) const HEADER_NAME: &str = "x-claudex-disabled-subagent-models";
const CONFIG_VERSION: u64 = 1;
const CONFIG_DIR_RELATIVE: &str = ".config/claudex";
const CONFIG_RELATIVE_PATH: &str = ".config/claudex/disabled-subagent-models.json";
const LOCAL_CONFIG_NAME: &str = "disabled-subagent-models.local.json";
const LOAD_ATTEMPTS: usize = 3;
#[cfg(not(test))]
const LOAD_RETRY: Duration = Duration::from_millis(10);
#[cfg(test)]
const LOAD_RETRY: Duration = Duration::ZERO;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ModelPolicy {
    version: u64,
    disabled_models: Vec<String>,
}

pub(crate) fn active_header() -> Result<Option<String>> {
    let path = config_path(
        std::env::var_os(CONFIG_ENV_NAME).as_deref(),
        std::env::var_os("HOME").as_deref(),
    )?;
    merged_header(
        &load_config(path.as_deref())?,
        std::env::var_os(ENV_NAME).as_deref(),
    )
}

/// Resolve the current machine policy for every incoming request. The launcher
/// snapshots this value into Claude's environment, but an already-running
/// Claude process may not have that snapshot; the HTTP boundary must still
/// enforce the current denylist.
pub(crate) fn active_models() -> Result<BTreeSet<String>> {
    active_header()?
        .map(|header| parse(&header))
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(crate) fn header_value(header: &Option<String>) -> &str {
    header.as_deref().unwrap_or_default()
}

pub(crate) fn apply_snapshot(command: &mut Command, header: &Option<String>) {
    command.env(RESOLVED_ENV_NAME, header_value(header));
}

fn short_hostname() -> Option<String> {
    let output = Command::new("hostname").arg("-s").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8(output.stdout).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

fn config_path(explicit: Option<&OsStr>, home: Option<&OsStr>) -> Result<Option<PathBuf>> {
    if let Some(explicit) = explicit {
        if explicit.is_empty() {
            bail!("{CONFIG_ENV_NAME} must not be empty");
        }
        let path = PathBuf::from(explicit);
        if !path.is_file() {
            bail!("{CONFIG_ENV_NAME} does not name a readable file");
        }
        return Ok(Some(path));
    }
    let Some(home) = home else {
        return Ok(None);
    };
    let config_dir = PathBuf::from(home).join(CONFIG_DIR_RELATIVE);
    if let Some(hostname) = short_hostname() {
        let hostname_local =
            config_dir.join(format!("disabled-subagent-models.{hostname}.local.json"));
        if hostname_local.is_file() {
            return Ok(Some(hostname_local));
        }
    }
    let shared_local = config_dir.join(LOCAL_CONFIG_NAME);
    if shared_local.is_file() {
        return Ok(Some(shared_local));
    }
    Ok(Some(PathBuf::from(home).join(CONFIG_RELATIVE_PATH)))
}

fn load_config(path: Option<&Path>) -> Result<BTreeSet<String>> {
    let Some(path) = path.filter(|path| path.is_file()) else {
        return Ok(BTreeSet::new());
    };
    load_config_from_reader(
        || {
            fs::read_to_string(path)
                .with_context(|| format!("read disabled SubAgent model config {}", path.display()))
        },
        path,
    )
}

fn load_config_from_reader(
    read: impl Fn() -> Result<String>,
    path: &Path,
) -> Result<BTreeSet<String>> {
    let mut last_error = None;
    for attempt in 0..LOAD_ATTEMPTS {
        match read().and_then(|contents| parse_policy_text(&contents, path)) {
            Ok(models) => return remember_last_good(models),
            Err(error) => {
                last_error = Some(error);
                sleep_before_policy_retry(attempt);
            }
        }
    }
    fallback_last_good(path, last_error.expect("load attempts produce an error"))
}

fn sleep_before_policy_retry(attempt: usize) {
    if attempt + 1 < LOAD_ATTEMPTS {
        std::thread::sleep(LOAD_RETRY);
    }
}

fn parse_policy_text(contents: &str, path: &Path) -> Result<BTreeSet<String>> {
    let policy: ModelPolicy = serde_json::from_str(contents).map_err(|error| {
        anyhow::anyhow!(
            "parse disabled SubAgent model config {}: {error}",
            path.display()
        )
    })?;
    if policy.version != CONFIG_VERSION {
        bail!("disabled SubAgent model config version must be {CONFIG_VERSION}");
    }
    if policy.models_are_invalid() {
        bail!("disabledModels must contain unique, valid exact model IDs");
    }
    Ok(policy.disabled_models.into_iter().collect())
}

fn remember_last_good(models: BTreeSet<String>) -> Result<BTreeSet<String>> {
    with_last_good(|slot| *slot = Some(models.clone()));
    Ok(models)
}

fn fallback_last_good(path: &Path, error: anyhow::Error) -> Result<BTreeSet<String>> {
    let cached = with_last_good(|slot| slot.clone());
    if let Some(models) = cached {
        eprintln!(
            "claudex: {error}; using last valid disabled SubAgent model config for {}",
            path.display()
        );
        return Ok(models);
    }
    Err(error)
}

fn with_last_good<R>(update: impl FnOnce(&mut Option<BTreeSet<String>>) -> R) -> R {
    #[cfg(test)]
    {
        thread_local! {
            static LAST_GOOD: std::cell::RefCell<Option<BTreeSet<String>>> =
                const { std::cell::RefCell::new(None) };
        }
        LAST_GOOD.with(|slot| update(&mut slot.borrow_mut()))
    }
    #[cfg(not(test))]
    {
        static LAST_GOOD: std::sync::Mutex<Option<BTreeSet<String>>> = std::sync::Mutex::new(None);
        let mut slot = LAST_GOOD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        update(&mut slot)
    }
}

impl ModelPolicy {
    fn models_are_invalid(&self) -> bool {
        self.disabled_models
            .iter()
            .any(|model| !valid_model_id(model))
            || self.disabled_models.iter().collect::<BTreeSet<_>>().len()
                != self.disabled_models.len()
    }
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

fn parse(value: &str) -> Result<BTreeSet<String>> {
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
