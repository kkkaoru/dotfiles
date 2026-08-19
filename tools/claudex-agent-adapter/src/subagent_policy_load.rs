use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::{
    CONFIG_DIR_RELATIVE, CONFIG_ENV_NAME, CONFIG_RELATIVE_PATH, CONFIG_VERSION, DENY_ALL_SENTINEL,
    LOAD_ATTEMPTS, LOAD_RETRY, LOCAL_CONFIG_NAME, valid_model_id,
};

const DEFAULT_DENYLIST_FILE_NAME: &str = "disabled-subagent-models.json";
const DEFAULT_DENYLIST_JSON: &str = "{\n  \"version\": 1,\n  \"disabledModels\": []\n}\n";

#[path = "subagent_policy_last_good.rs"]
mod last_good;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct ModelPolicy {
    version: u64,
    disabled_models: Vec<String>,
}

enum FileProbe {
    File,
    Missing,
    NotFile,
    Unreadable,
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
        if matches!(probe_file(&path), FileProbe::NotFile) {
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
        if prefer_dedicated(&hostname_local) {
            return Ok(Some(hostname_local));
        }
    }
    let shared_local = config_dir.join(LOCAL_CONFIG_NAME);
    if prefer_dedicated(&shared_local) {
        return Ok(Some(shared_local));
    }
    Ok(Some(PathBuf::from(home).join(CONFIG_RELATIVE_PATH)))
}

fn prefer_dedicated(path: &Path) -> bool {
    match probe_file(path) {
        FileProbe::File | FileProbe::Unreadable => true,
        FileProbe::Missing | FileProbe::NotFile => false,
    }
}

fn probe_file(path: &Path) -> FileProbe {
    let mut last_unreadable = false;
    for attempt in 0..LOAD_ATTEMPTS {
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => return FileProbe::File,
            Ok(_) => return FileProbe::NotFile,
            Err(error) if error.kind() == ErrorKind::NotFound => return FileProbe::Missing,
            Err(error) if io_error_is_transient(&error) => {
                last_unreadable = true;
                sleep_before_policy_retry(attempt);
            }
            Err(_) => return FileProbe::Unreadable,
        }
    }
    if last_unreadable {
        FileProbe::Unreadable
    } else {
        FileProbe::Missing
    }
}

pub(super) fn load_config(path: Option<&Path>) -> BTreeSet<String> {
    let Some(path) = path else {
        return BTreeSet::new();
    };
    load_config_from_reader(
        || {
            fs::read_to_string(path)
                .with_context(|| format!("read disabled SubAgent model config {}", path.display()))
        },
        path,
    )
}

fn is_optional_tracked(path: &Path) -> bool {
    path.file_name() == Some(OsStr::new(DEFAULT_DENYLIST_FILE_NAME))
        && std::env::var_os(CONFIG_ENV_NAME).is_none()
}

fn missing_denylist(path: &Path) -> BTreeSet<String> {
    if let Some(models) = cached_last_good(path) {
        remember_warning(format!(
            "denylist file missing; using last-known-good for {}",
            path.display()
        ));
        return models;
    }
    if is_optional_tracked(path) {
        seed_canonical_denylist(path);
        return BTreeSet::new();
    }
    fail_closed(
        path,
        format!("denylist file missing for {}", path.display()),
    )
}

fn cached_last_good(path: &Path) -> Option<BTreeSet<String>> {
    let memory = with_cache(|cache| {
        if cache.last_good_source.as_deref() == Some(path) {
            cache.last_good.clone()
        } else {
            None
        }
    });
    memory.or_else(|| last_good::restore(path))
}

fn io_not_found(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == ErrorKind::NotFound)
    })
}

fn io_error_is_transient(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::Interrupted | ErrorKind::WouldBlock | ErrorKind::TimedOut
    ) || os_emfile(error.raw_os_error())
}

fn os_emfile(code: Option<i32>) -> bool {
    #[cfg(unix)]
    {
        matches!(code, Some(libc::EMFILE | libc::ENFILE | libc::EAGAIN))
    }
    #[cfg(not(unix))]
    {
        let _ = code;
        false
    }
}

fn seed_canonical_denylist(path: &Path) {
    if path.file_name() != Some(OsStr::new(DEFAULT_DENYLIST_FILE_NAME)) {
        return;
    }
    if let Err(error) = write_canonical_denylist(path) {
        remember_warning(format!(
            "could not create default denylist {}: {error}",
            path.display()
        ));
    }
}

fn write_canonical_denylist(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => Ok(()),
        Ok(_) => Ok(fs::write(path, DEFAULT_DENYLIST_JSON)?),
        Err(error) if error.kind() == ErrorKind::NotFound => write_new_canonical_denylist(path),
        Err(error) => Err(error.into()),
    }
}

fn write_new_canonical_denylist(path: &Path) -> Result<()> {
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => Ok(file.write_all(DEFAULT_DENYLIST_JSON.as_bytes())?),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn load_config_from_reader(
    read: impl Fn() -> Result<String>,
    path: &Path,
) -> BTreeSet<String> {
    let mut last_error = None;
    for attempt in 0..LOAD_ATTEMPTS {
        match read() {
            Err(error) if io_not_found(&error) => return missing_denylist(path),
            Ok(contents) if contents.trim().is_empty() => return missing_denylist(path),
            Ok(contents) => match parse_policy_text(&contents, path) {
                Ok(models) => return remember_last_good(path, models),
                Err(error) => last_error = Some(error),
            },
            Err(error) => last_error = Some(error),
        }
        sleep_before_policy_retry(attempt);
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

pub(super) fn remember_last_good(path: &Path, models: BTreeSet<String>) -> BTreeSet<String> {
    with_cache(|cache| {
        cache.last_good_source = Some(path.to_path_buf());
        cache.last_good = Some(models.clone());
        cache.warning = None;
    });
    if !models.contains(DENY_ALL_SENTINEL) {
        last_good::persist(path, &models);
    }
    models
}

pub(super) fn fallback_last_good(path: &Path, error: anyhow::Error) -> BTreeSet<String> {
    if let Some(models) = cached_last_good(path) {
        remember_warning(format!(
            "{error}; using last-known-good denylist for {}",
            path.display()
        ));
        return models;
    }
    if is_optional_tracked(path) {
        remember_warning(format!(
            "{error}; optional tracked denylist unavailable for {}",
            path.display()
        ));
        return BTreeSet::new();
    }
    fail_closed(
        path,
        format!(
            "{error}; denylist unavailable at cold start for {}; refusing allow-all",
            path.display()
        ),
    )
}

fn fail_closed(path: &Path, message: String) -> BTreeSet<String> {
    let _ = path;
    remember_warning(message);
    BTreeSet::from([DENY_ALL_SENTINEL.to_owned()])
}

struct DenylistCache {
    last_good_source: Option<PathBuf>,
    last_good: Option<BTreeSet<String>>,
    warning: Option<String>,
}

impl DenylistCache {
    const fn empty() -> Self {
        Self {
            last_good_source: None,
            last_good: None,
            warning: None,
        }
    }
}

fn remember_warning(message: String) {
    eprintln!("claudex: {message}");
    with_cache(|cache| cache.warning = Some(message));
}

pub(crate) fn surface_denylist_error(error: anyhow::Error) -> BTreeSet<String> {
    remember_warning(error.to_string());
    BTreeSet::from([DENY_ALL_SENTINEL.to_owned()])
}

pub(crate) fn denylist_load_warning() -> Option<String> {
    with_cache(|cache| cache.warning.clone())
}

#[cfg(test)]
pub(super) fn clear_denylist_cache() {
    with_cache(|cache| *cache = DenylistCache::empty());
    last_good::reset_test_file();
}

#[cfg(test)]
pub(super) fn clear_memory_keep_disk_last_good() {
    with_cache(|cache| *cache = DenylistCache::empty());
    last_good::keep_test_file();
}

fn with_cache<R>(update: impl FnOnce(&mut DenylistCache) -> R) -> R {
    #[cfg(test)]
    {
        thread_local! {
            static CACHE: std::cell::RefCell<DenylistCache> =
                const { std::cell::RefCell::new(DenylistCache::empty()) };
        }
        CACHE.with(|slot| update(&mut slot.borrow_mut()))
    }
    #[cfg(not(test))]
    {
        static CACHE: std::sync::Mutex<DenylistCache> =
            std::sync::Mutex::new(DenylistCache::empty());
        let mut slot = CACHE
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
