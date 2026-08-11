use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::{
    CONFIG_DIR_RELATIVE, CONFIG_ENV_NAME, CONFIG_RELATIVE_PATH, CONFIG_VERSION, LOAD_ATTEMPTS,
    LOAD_RETRY, LOCAL_CONFIG_NAME, valid_model_id,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct ModelPolicy {
    version: u64,
    disabled_models: Vec<String>,
}

pub(super) fn short_hostname() -> Option<String> {
    let output = Command::new("hostname").arg("-s").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8(output.stdout).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

pub(super) fn config_path(
    explicit: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Result<Option<PathBuf>> {
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

pub(super) fn load_config(path: Option<&Path>) -> Result<BTreeSet<String>> {
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

pub(super) fn load_config_from_reader(
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

pub(super) fn sleep_before_policy_retry(attempt: usize) {
    if attempt + 1 < LOAD_ATTEMPTS {
        std::thread::sleep(LOAD_RETRY);
    }
}

pub(super) fn parse_policy_text(contents: &str, path: &Path) -> Result<BTreeSet<String>> {
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

pub(super) fn remember_last_good(models: BTreeSet<String>) -> Result<BTreeSet<String>> {
    with_last_good(|slot| *slot = Some(models.clone()));
    Ok(models)
}

pub(super) fn fallback_last_good(path: &Path, error: anyhow::Error) -> Result<BTreeSet<String>> {
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

pub(super) fn with_last_good<R>(update: impl FnOnce(&mut Option<BTreeSet<String>>) -> R) -> R {
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
    pub(super) fn models_are_invalid(&self) -> bool {
        self.disabled_models
            .iter()
            .any(|model| !valid_model_id(model))
            || self.disabled_models.iter().collect::<BTreeSet<_>>().len()
                != self.disabled_models.len()
    }
}
