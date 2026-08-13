//! Provider configuration loading, validation, and the disabled-model policy.

use anyhow::{Result, bail};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const OPENCODE_GO_DEFAULT_MODEL: &str = "opencode-go/deepseek-v4-pro";
pub const OPENCODE_GO_DEFAULT_USAGE_PROVIDER: &str = "opencodego";
/// Bump when worker-selection semantics change so a cached context cannot
/// retain the old ordering or exclusion rules for the routing-cache TTL.
pub const ROUTING_CACHE_VERSION: i64 = 11;

pub fn default_advisor() -> Value {
    serde_json::json!({
        "agent": "custom-advisor",
        "model": "claude-fable-5",
        "effort": "xhigh"
    })
}

/// Resolved home directory and repository root used for config discovery.
pub struct Paths {
    pub home: PathBuf,
    pub cache_dir: PathBuf,
    pub repository_root: PathBuf,
}

impl Paths {
    pub fn discover(_requested_config: Option<&Path>) -> Result<Self> {
        let home = home_dir()?;
        Ok(Self {
            cache_dir: cache_dir(&home),
            home,
            repository_root: repository_root(),
        })
    }
}

/// Resolve the cache root shared with the session policy hook. An explicit
/// override is authoritative in both binaries; otherwise the root is under
/// the resolved HOME directory.
pub fn cache_dir(home: &Path) -> PathBuf {
    std::env::var_os("CLAUDEX_CACHE_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cache/claudex"))
}

fn home_dir() -> Result<PathBuf> {
    match std::env::var_os("HOME") {
        Some(value) if !value.is_empty() => Ok(PathBuf::from(value)),
        _ => bail!("HOME environment variable is required"),
    }
}

/// Repository root relative to the installed binary or an explicit override.
///
/// The Python module derived this from its own file path; the Rust binary
/// resolves it from `CLAUDEX_REPOSITORY_ROOT` when set, then the ghq checkout,
/// so the repository-local config/denylist fallbacks stay reachable.
fn repository_root() -> PathBuf {
    if let Some(root) = std::env::var_os("CLAUDEX_REPOSITORY_ROOT")
        && !root.is_empty()
    {
        return PathBuf::from(root);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let checkout = PathBuf::from(home).join("ghq/github.com/kkkaoru/dotfiles");
        if checkout.is_dir() {
            return checkout;
        }
    }
    PathBuf::from(".")
}

fn expand_user(value: &str, home: &Path) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        return home.join(rest);
    }
    if value == "~" {
        return home.to_path_buf();
    }
    PathBuf::from(value)
}

pub fn provider_config_path(requested: Option<&Path>, paths: &Paths) -> PathBuf {
    if let Some(path) = requested {
        return path.to_path_buf();
    }
    if let Ok(configured) = std::env::var("CLAUDEX_PROVIDER_CONFIG")
        && !configured.is_empty()
    {
        return expand_user(&configured, &paths.home);
    }
    let installed = paths.home.join(".config/claudex/providers.json");
    if installed.is_file() {
        installed
    } else {
        paths.repository_root.join(".config/claudex/providers.json")
    }
}

pub fn disabled_models_path(requested: Option<&Path>, paths: &Paths) -> Result<PathBuf> {
    if let Some(path) = requested {
        return Ok(path.to_path_buf());
    }
    if let Some(configured) = std::env::var_os("CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG") {
        let configured = configured.to_string_lossy().into_owned();
        if configured.is_empty() {
            bail!("CLAUDEX_DISABLED_SUBAGENT_MODELS_CONFIG must not be empty");
        }
        return Ok(expand_user(&configured, &paths.home));
    }
    let config_dir = paths.home.join(".config/claudex");
    if let Some(hostname) = short_hostname() {
        let hostname_local =
            config_dir.join(format!("disabled-subagent-models.{hostname}.local.json"));
        if hostname_local.is_file() {
            return Ok(hostname_local);
        }
    }
    let shared_local = config_dir.join("disabled-subagent-models.local.json");
    if shared_local.is_file() {
        return Ok(shared_local);
    }
    let installed = config_dir.join("disabled-subagent-models.json");
    if installed.is_file() {
        Ok(installed)
    } else {
        Ok(paths
            .repository_root
            .join(".config/claudex/disabled-subagent-models.json"))
    }
}

#[cfg(unix)]
fn short_hostname() -> Option<String> {
    let mut buffer = [0_u8; 256];
    // SAFETY: `buffer` is writable for its declared length and remains alive
    // for the duration of this libc call.
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return None;
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    let full = std::str::from_utf8(&buffer[..end]).ok()?.trim();
    let short = full.split('.').next().unwrap_or(full);
    (!short.is_empty()).then(|| short.to_owned())
}

#[cfg(not(unix))]
fn short_hostname() -> Option<String> {
    let full = std::env::var("HOSTNAME").ok()?;
    let short = full.trim().split('.').next().unwrap_or_default();
    (!short.is_empty()).then(|| short.to_owned())
}

/// The validated, normalized routing configuration.
pub struct Config {
    /// The full validated config with the `providers`/`nativeWorkers`/`advisor`
    /// keys normalized, preserved for `configuration_key` hashing.
    pub raw: Value,
    pub providers: Vec<Value>,
    pub native_workers: Vec<Value>,
    pub fallback: Value,
    pub advisor: Value,
}

pub fn valid_model_id(model: &Value) -> bool {
    let Some(text) = model.as_str() else {
        return false;
    };
    !text.is_empty() && text.is_ascii() && text.chars().all(|c| ('!'..='~').contains(&c))
}

fn nonempty_str(value: &Value, field: &str) -> bool {
    value
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty())
}

fn valid_choice(choice: &Value) -> bool {
    choice.is_object()
        && ["agent", "model", "effort"]
            .iter()
            .all(|field| nonempty_str(choice, field))
}

fn is_integer(value: &Value) -> bool {
    value.is_i64() || value.is_u64()
}

pub fn valid_request_budget(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let keys: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    if keys != BTreeSet::from(["estimatedRequests", "windowMinutes", "usageWindow"]) {
        return false;
    }
    let requests = &object["estimatedRequests"];
    let minutes = &object["windowMinutes"];
    let window = &object["usageWindow"];
    is_integer(requests)
        && requests.as_i64().is_some_and(|value| value > 0)
        && is_integer(minutes)
        && minutes.as_i64().is_some_and(|value| value > 0)
        && valid_model_id(window)
}

fn valid_provider(provider: &Value) -> bool {
    let Some(object) = provider.as_object() else {
        return false;
    };
    if !["id", "agent", "defaultModel", "effort", "backend"]
        .iter()
        .all(|field| nonempty_str(provider, field))
    {
        return false;
    }
    if let Some(subagent) = object.get("subagentModel")
        && !valid_model_id(subagent)
    {
        return false;
    }
    match object.get("maxConcurrency") {
        None | Some(Value::Null) => {}
        Some(value) => {
            let valid = !value.is_boolean() && value.as_i64().is_some_and(|maximum| maximum > 0);
            if !valid {
                return false;
            }
        }
    }
    match object.get("requestBudget") {
        None | Some(Value::Null) => {}
        Some(budget) => {
            let default_model = object
                .get("defaultModel")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !(default_model == OPENCODE_GO_DEFAULT_MODEL
                || default_model.starts_with("opencode-go/"))
                || !object
                    .get("usageProvider")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .eq_ignore_ascii_case(OPENCODE_GO_DEFAULT_USAGE_PROVIDER)
                || !valid_request_budget(budget)
            {
                return false;
            }
        }
    }
    match object.get("usageWeeklyWindowId") {
        None | Some(Value::Null) => true,
        Some(value) => value.as_str().is_some_and(|text| !text.is_empty()),
    }
}

pub fn load_config(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path)?;
    let mut config: Value = serde_json::from_str(&text)?;
    if config.get("version").and_then(Value::as_i64) != Some(1) {
        bail!("provider config version must be 1");
    }
    let providers_raw = config
        .get("providers")
        .and_then(Value::as_array)
        .filter(|providers| !providers.is_empty())
        .ok_or_else(|| anyhow::anyhow!("provider config must contain providers"))?;
    let enabled: Vec<Value> = providers_raw
        .iter()
        .filter(|provider| {
            provider
                .get("enabled")
                .is_none_or(|value| value.as_bool().unwrap_or(false))
        })
        .cloned()
        .collect();
    if enabled.is_empty() || !enabled.iter().all(valid_provider) {
        bail!("provider config contains an invalid enabled provider");
    }
    let enabled_ids: BTreeSet<String> = enabled
        .iter()
        .filter_map(|provider| provider.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    let main_providers = match config.get("mainProviders") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items.clone(),
        Some(_) => bail!("mainProviders must name distinct enabled providers"),
    };
    let main_names: Vec<&str> = main_providers
        .iter()
        .map(|value| value.as_str().unwrap_or_default())
        .collect();
    let distinct: BTreeSet<&str> = main_names.iter().copied().collect();
    if main_providers.iter().any(|value| !value.is_string())
        || distinct.len() != main_names.len()
        || main_names.iter().any(|name| !enabled_ids.contains(*name))
    {
        bail!("mainProviders must name distinct enabled providers");
    }
    let fallback = config.get("fallback").cloned().unwrap_or(Value::Null);
    if !valid_choice(&fallback) {
        bail!("provider config contains an invalid fallback");
    }
    let native_workers = validate_native_workers(&config, &enabled)?;
    let advisor = config
        .get("advisor")
        .cloned()
        .unwrap_or_else(default_advisor);
    if !valid_choice(&advisor) {
        bail!("provider config contains an invalid advisor");
    }
    if let Some(object) = config.as_object_mut() {
        object.insert("mainProviders".into(), Value::Array(main_providers));
        object.insert("providers".into(), Value::Array(enabled.clone()));
        object.insert("nativeWorkers".into(), Value::Array(native_workers.clone()));
        object.insert("advisor".into(), advisor.clone());
    }
    Ok(Config {
        raw: config,
        providers: enabled,
        native_workers,
        fallback,
        advisor,
    })
}

fn validate_native_workers(config: &Value, enabled: &[Value]) -> Result<Vec<Value>> {
    let native_workers = match config.get("nativeWorkers") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items.clone(),
        Some(_) => bail!("provider config contains invalid nativeWorkers"),
    };
    let agents: BTreeSet<&str> = native_workers
        .iter()
        .filter_map(|worker| worker.get("agent").and_then(Value::as_str))
        .collect();
    let usage_provider_invalid = native_workers.iter().any(|worker| {
        worker
            .get("usageProvider")
            .is_some_and(|value| value.as_str().is_none_or(str::is_empty))
    });
    if !native_workers.iter().all(valid_choice)
        || usage_provider_invalid
        || agents.len() != native_workers.len()
    {
        bail!("provider config contains invalid nativeWorkers");
    }
    let provider_agents: BTreeSet<&str> = enabled
        .iter()
        .filter_map(|provider| provider.get("agent").and_then(Value::as_str))
        .collect();
    if native_workers.iter().any(|worker| {
        worker
            .get("agent")
            .and_then(Value::as_str)
            .is_some_and(|agent| provider_agents.contains(agent))
    }) {
        bail!("nativeWorkers agent values must not overlap enabled providers");
    }
    Ok(native_workers)
}

pub fn load_disabled_models(path: &Path) -> Result<BTreeSet<String>> {
    Ok(load_disabled_models_policy(path).models())
}

#[path = "config_denylist.rs"]
mod denylist_policy;
#[cfg(test)]
pub use denylist_policy::enabled_denylist_conflicts;
pub use denylist_policy::load_disabled_models_policy;

pub fn parse_environment_models(raw: &str) -> Result<BTreeSet<String>> {
    let mut models = BTreeSet::new();
    for item in raw.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !valid_model_id(&Value::String(trimmed.to_owned())) {
            bail!("environment contains an invalid model ID");
        }
        models.insert(trimmed.to_owned());
    }
    Ok(models)
}

/// Bind cached capacity decisions to the config and terminal model policy.
pub fn configuration_key(config: &Value, disabled_models: &BTreeSet<String>) -> String {
    let mut payload = Map::new();
    payload.insert("cacheVersion".into(), Value::from(ROUTING_CACHE_VERSION));
    payload.insert("config".into(), config.clone());
    payload.insert(
        "disabledSubagentModels".into(),
        Value::Array(disabled_models.iter().cloned().map(Value::from).collect()),
    );
    let compact = util_canonical(&Value::Object(payload));
    let mut hasher = Sha256::new();
    hasher.update(compact.as_bytes());
    hex::encode(hasher.finalize())
}

/// Serialize with sorted keys and compact separators to match Python's
/// `json.dumps(..., sort_keys=True, separators=(",", ":"))`.
fn util_canonical(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => write_canonical_object(map, out),
        Value::Array(items) => write_canonical_array(items, out),
        other => out.push_str(&serde_json::to_string(other).unwrap_or_default()),
    }
}

fn write_canonical_object(map: &Map<String, Value>, out: &mut String) {
    out.push('{');
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    for (index, key) in keys.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(key).unwrap_or_default());
        out.push(':');
        write_canonical(&map[*key], out);
    }
    out.push('}');
}

fn write_canonical_array(items: &[Value], out: &mut String) {
    out.push('[');
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_canonical(item, out);
    }
    out.push(']');
}
