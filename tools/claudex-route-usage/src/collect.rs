//! Provider usage collection, Ollama probes, daemon health, and memory.

use crate::config::{Config, Paths, valid_model_id};
use crate::process::{
    Deadline, run_before, run_before_with_input, run_with_timeout, run_with_timeout_with_input,
};
use crate::routing::quota::provider_status;
use crate::routing::workers::worker;
use crate::routing::{memory_management_enabled, memory_pressure_thresholds, pressure_level};
use crate::util::{number_f64, python_round, valid_percentage};
use anyhow::{Result, bail};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use url::Url;

pub const REQUEST_TIMEOUT_SECONDS: u64 = 5;
pub const SUBPROCESS_GRACE_SECONDS: u64 = 2;
pub const USAGE_COMMAND_TIMEOUT_SECONDS: u64 = 45;
pub const CLAUDE_OAUTH_ENSURE_TIMEOUT_SECONDS: u64 = 45;
pub const DAEMON_HEALTH_TIMEOUT_SECONDS: u64 = 2;
pub const MEMORY_COMMAND_TIMEOUT_SECONDS: u64 = 5;
pub const OLLAMA_USAGE_PROVIDER: &str = "ollama";
pub const OLLAMA_BASE_URL_ENV: &str = "CLAUDEX_OLLAMA_BASE_URL";
pub const DEFAULT_OLLAMA_BASE_URL: &str = "http://127.0.0.1:11434";
/// Ollama Cloud's balance API, and the key herdr-app's agent-limits mod uses: the variable,
/// else ~/.codex/.env, else ~/.env (the dotfiles-managed files every machine has), else pi's own
/// login in ~/.pi/agent/auth.json.
pub const OLLAMA_BALANCE_URL: &str = "https://ollama.com/api/balance";
pub const OLLAMA_API_KEY_ENV: &str = "OLLAMA_API_KEY";
const OLLAMA_API_KEY_FILES: &[&str] = &[".codex/.env", ".env"];
const PI_AUTH_FILE: &str = ".pi/agent/auth.json";
const PI_OLLAMA_PROVIDER: &str = "ollama-cloud";
/// CodexBar reads OpenCode Go's usage API with its own key, which pi's login also holds.
pub const OPENCODE_API_KEY_ENV: &str = "OPENCODE_API_KEY";
const PI_OPENCODE_PROVIDER: &str = "opencode-go";
const FIVE_HOUR_MINUTES: i64 = 300;
const SEVEN_DAY_MINUTES: i64 = 10_080;
pub const DAEMON_HEALTH_URL_ENV: &str = "CLAUDEX_DAEMON_HEALTH_URL";
pub const ANTHROPIC_BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";
pub const DEFAULT_DAEMON_HEALTH_URL: &str = "http://127.0.0.1:8318/health";
const OAUTH_SCRIPT: &[u8] = include_bytes!("../../../scripts/ensure-claude-oauth.sh");
const SAFE_HELPER_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

pub fn unavailable_usage_entry(provider: &str) -> Value {
    serde_json::json!({
        "provider": provider,
        "available": false,
        "reason": "usage-unavailable",
    })
}

pub fn strict_json_array(output: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(output)?;
    if !value.is_array() {
        bail!("Codexbar output must be a JSON array");
    }
    Ok(value)
}

fn run_codexbar_before(program: &str, deadline: Option<Deadline>) -> Result<Value> {
    let mut command = Command::new(program);
    command.args(["usage", "--json"]);
    if let Some(key) = opencode_api_key() {
        command.env(OPENCODE_API_KEY_ENV, key);
    }
    let (_code, stdout, _stderr) = run_collector(
        command,
        deadline,
        Duration::from_secs(USAGE_COMMAND_TIMEOUT_SECONDS),
    )?;
    strict_json_array(&stdout)
}

fn ensure_claude_oauth(deadline: Option<Deadline>) {
    let Some(deadline) = deadline else {
        return;
    };
    let Ok(command) = oauth_command() else {
        return;
    };
    let timeout = Duration::from_secs(CLAUDE_OAUTH_ENSURE_TIMEOUT_SECONDS);
    if deadline.remaining().is_none() {
        return;
    }
    let result = run_before_with_input(command, deadline, timeout, OAUTH_SCRIPT);
    if let Err(error) = result {
        eprintln!("claudex-route-usage: Claude OAuth ensure skipped: {error}");
    }
}

fn oauth_command() -> Result<Command> {
    let bash = crate::trusted::executable("/bin/bash")?;
    let home = env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is required"))?;
    let mut command = Command::new(bash);
    command
        .args(["-s", "--", "--refresh-within", "7200"])
        .env_clear()
        .env("HOME", home)
        .env("PATH", SAFE_HELPER_PATH)
        .current_dir("/");
    Ok(command)
}

#[cfg(test)]
fn oauth_contract() -> (&'static [u8], Vec<String>) {
    (
        OAUTH_SCRIPT,
        vec![
            "-s".to_owned(),
            "--".to_owned(),
            "--refresh-within".to_owned(),
            "7200".to_owned(),
        ],
    )
}

fn ollama_usage_entry_before(
    curl_program: &str,
    provider: &str,
    model: &str,
    deadline: Option<Deadline>,
) -> Value {
    let base_url =
        env::var(OLLAMA_BASE_URL_ENV).unwrap_or_else(|_| DEFAULT_OLLAMA_BASE_URL.to_owned());
    let base_url = base_url.trim_end_matches('/');
    let Ok(parsed) = Url::parse(base_url) else {
        return unavailable_usage_entry(provider);
    };
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return unavailable_usage_entry(provider);
    }
    let mut command = Command::new(curl_program);
    command.args([
        "--fail",
        "--silent",
        "--show-error",
        "--max-time",
        &REQUEST_TIMEOUT_SECONDS.to_string(),
        &format!("{base_url}/api/tags"),
    ]);
    let timeout = Duration::from_secs(REQUEST_TIMEOUT_SECONDS + SUBPROCESS_GRACE_SECONDS);
    if deadline.is_some_and(|deadline| deadline.remaining().is_none()) {
        return unavailable_usage_entry(provider);
    }
    let Ok((code, stdout, _)) = run_collector(command, deadline, timeout) else {
        return unavailable_usage_entry(provider);
    };
    if code != 0 {
        return unavailable_usage_entry(provider);
    }
    let Ok(payload) = serde_json::from_str::<Value>(&stdout) else {
        return unavailable_usage_entry(provider);
    };
    let Some(models) = payload.get("models").and_then(Value::as_array) else {
        return unavailable_usage_entry(provider);
    };
    let found = models.iter().any(|item| {
        item.get("name").and_then(Value::as_str) == Some(model)
            || item.get("model").and_then(Value::as_str) == Some(model)
    });
    if !found {
        return unavailable_usage_entry(provider);
    }
    // Local /api/tags only proves the model is reachable; the cloud quotas come from
    // `ollama_balance_entry_before`. Never invent maxUsedPercent=0 (that used to rank as 100%
    // weekly headroom and beat real CodexBar meters).
    serde_json::json!({
        "provider": provider,
        "available": true,
        "reason": "available-ollama-api-only",
    })
}

/// Ollama Cloud's `/api/balance` as a CodexBar-shaped entry, so routing reads the same 5h and
/// weekly meters as every other provider, or None when no key is configured or curl got no answer.
fn ollama_balance_entry_before(
    curl_program: &str,
    provider: &str,
    deadline: Option<Deadline>,
) -> Option<Value> {
    let key = ollama_api_key()?;
    let mut command = Command::new(curl_program);
    command.args([
        "--fail",
        "--silent",
        "--show-error",
        "--max-time",
        &REQUEST_TIMEOUT_SECONDS.to_string(),
        "--header",
        "@-",
        OLLAMA_BALANCE_URL,
    ]);
    let timeout = Duration::from_secs(REQUEST_TIMEOUT_SECONDS + SUBPROCESS_GRACE_SECONDS);
    if deadline.is_some_and(|deadline| deadline.remaining().is_none()) {
        return None;
    }
    // The header goes in on stdin, so the key stays out of every process's arguments.
    let header = format!("Authorization: Bearer {key}\n");
    let (code, stdout, _) =
        run_collector_with_input(command, deadline, timeout, header.as_bytes()).ok()?;
    if code != 0 {
        return None;
    }
    ollama_balance_windows(&stdout).map(|usage| {
        serde_json::json!({
            "provider": provider,
            "usage": usage,
        })
    })
}

/// `/api/balance` -> CodexBar's primary (session) and secondary (weekly) windows.
fn ollama_balance_windows(stdout: &str) -> Option<Value> {
    let payload: Value = serde_json::from_str(stdout).ok()?;
    let included = payload.get("included")?;
    let mut usage = serde_json::Map::new();
    for (slot, window, minutes) in [
        ("primary", "session", FIVE_HOUR_MINUTES),
        ("secondary", "weekly", SEVEN_DAY_MINUTES),
    ] {
        let Some(used) = included
            .get(window)
            .and_then(|window| window.get("remaining_percent"))
            .and_then(remaining_as_used_percent)
        else {
            continue;
        };
        usage.insert(
            slot.to_owned(),
            serde_json::json!({ "windowMinutes": minutes, "usedPercent": used }),
        );
    }
    (!usage.is_empty()).then_some(Value::Object(usage))
}

fn remaining_as_used_percent(remaining: &Value) -> Option<f64> {
    if !valid_percentage(remaining) {
        return None;
    }
    number_f64(remaining)
        .filter(|percent| *percent <= 100.0)
        .map(|percent| python_round(100.0 - percent, 6))
}

fn ollama_api_key() -> Option<String> {
    let home = env::var_os("HOME")?;
    let home = Path::new(&home);
    env_key(OLLAMA_API_KEY_ENV)
        .or_else(|| dotenv_key(home))
        .or_else(|| pi_provider_key(home, PI_OLLAMA_PROVIDER))
}

fn opencode_api_key() -> Option<String> {
    let home = env::var_os("HOME")?;
    env_key(OPENCODE_API_KEY_ENV)
        .or_else(|| pi_provider_key(Path::new(&home), PI_OPENCODE_PROVIDER))
}

fn env_key(name: &str) -> Option<String> {
    let key = env::var(name).ok()?;
    Some(unquoted(&key))
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
}

fn pi_provider_key(home: &Path, provider: &str) -> Option<String> {
    let text = std::fs::read_to_string(home.join(PI_AUTH_FILE)).ok()?;
    let auth: Value = serde_json::from_str(&text).ok()?;
    Some(unquoted(auth.get(provider)?.get("key")?.as_str()?))
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
}

fn dotenv_key(home: &Path) -> Option<String> {
    OLLAMA_API_KEY_FILES.iter().find_map(|name| {
        let text = std::fs::read_to_string(home.join(name)).ok()?;
        dotenv_value(&text, OLLAMA_API_KEY_ENV)
    })
}

fn dotenv_value(text: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    text.lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .map(unquoted)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn unquoted(value: &str) -> &str {
    value.trim().trim_matches(['"', '\''])
}

fn collect_codexbar_report_before(
    codexbar_program: &str,
    codexbar_names: &BTreeSet<String>,
    deadline: Option<Deadline>,
) -> Vec<Value> {
    if codexbar_names.contains("claude") {
        ensure_claude_oauth(deadline);
    }
    match run_codexbar_before(codexbar_program, deadline) {
        Ok(Value::Array(entries)) => entries,
        _ => codexbar_names
            .iter()
            .map(|name| unavailable_usage_entry(name))
            .collect(),
    }
}

pub fn collect_usage_before(
    config: &Config,
    codexbar_program: &str,
    curl_program: &str,
    _paths: &Paths,
    _now: f64,
    disabled_models: &BTreeSet<String>,
    deadline: Option<Deadline>,
) -> Vec<Value> {
    let providers = providers_for_collection(config, disabled_models);
    if providers.is_empty() {
        return config
            .providers
            .iter()
            .filter_map(|provider| provider.get("id").and_then(Value::as_str))
            .map(unavailable_usage_entry)
            .collect();
    }
    let mut report = collect_codexbar_report_before(
        codexbar_program,
        &codexbar_usage_names(&providers, config),
        deadline,
    );
    let fallback_providers = ollama_fallback_providers(&providers, &report);
    if fallback_providers.is_empty() {
        return report;
    }
    for (usage_provider, entry) in
        probe_ollama_providers(&fallback_providers, curl_program, deadline)
    {
        replace_usage_entry(&mut report, &usage_provider, entry);
    }
    report
}

/// Disabled models remain capacity-observation targets; policy is applied only
/// when the collected report is converted into routable workers.
fn providers_for_collection<'a>(
    config: &'a Config,
    _disabled_models: &BTreeSet<String>,
) -> Vec<&'a Value> {
    config.providers.iter().collect()
}

fn codexbar_usage_names(providers: &[&Value], config: &Config) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = providers
        .iter()
        .filter_map(|provider| provider.get("usageProvider").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    // Native Claude workers (haiku-search / sonnet) also need CodexBar's claude
    // entry even when no capacity-managed provider declares usageProvider=claude.
    for worker_cfg in &config.native_workers {
        if let Some(name) = worker_cfg.get("usageProvider").and_then(Value::as_str)
            && !name.is_empty()
        {
            names.insert(name.to_owned());
        }
    }
    names
}

/// Ollama providers whose CodexBar entry is missing or unusable, so the balance API (or the
/// local API when no key is configured) is the only remaining capacity signal.
fn ollama_fallback_providers<'a>(providers: &[&'a Value], report: &[Value]) -> Vec<&'a Value> {
    let snapshot = Value::Array(report.to_vec());
    providers
        .iter()
        .copied()
        .filter(|provider| {
            provider
                .get("usageProvider")
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case(OLLAMA_USAGE_PROVIDER))
        })
        .filter(|provider| {
            let usage_provider = provider
                .get("usageProvider")
                .and_then(Value::as_str)
                .unwrap_or_default();
            matches!(
                provider_status(&snapshot, usage_provider)
                    .get("reason")
                    .and_then(Value::as_str),
                Some("missing" | "unknown" | "usage-unavailable")
            )
        })
        .collect()
}

fn probe_ollama_providers(
    providers: &[&Value],
    curl_program: &str,
    deadline: Option<Deadline>,
) -> Vec<(String, Value)> {
    providers
        .iter()
        .map(|provider| {
            let usage_provider = provider
                .get("usageProvider")
                .and_then(Value::as_str)
                .unwrap_or(OLLAMA_USAGE_PROVIDER)
                .to_owned();
            let worker = worker(provider);
            let model = worker
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let entry = ollama_balance_entry_before(curl_program, &usage_provider, deadline)
                .unwrap_or_else(|| {
                    ollama_usage_entry_before(curl_program, &usage_provider, model, deadline)
                });
            (usage_provider, entry)
        })
        .collect()
}

fn run_collector(
    command: Command,
    deadline: Option<Deadline>,
    maximum: Duration,
) -> Result<(i32, String, String)> {
    match deadline {
        Some(deadline) => run_before(command, deadline, maximum),
        None => run_with_timeout(command, maximum),
    }
}

fn run_collector_with_input(
    command: Command,
    deadline: Option<Deadline>,
    maximum: Duration,
    input: &[u8],
) -> Result<(i32, String, String)> {
    match deadline {
        Some(deadline) => run_before_with_input(command, deadline, maximum, input),
        None => run_with_timeout_with_input(command, maximum, input),
    }
}

fn replace_usage_entry(report: &mut Vec<Value>, usage_provider: &str, entry: Value) {
    report.retain(|item| {
        item.get("provider")
            .and_then(Value::as_str)
            .is_none_or(|name| !name.eq_ignore_ascii_case(usage_provider))
    });
    report.push(entry);
}

pub fn validate_daemon_health_url(value: &str) -> Result<String> {
    let parsed = Url::parse(value)?;
    if parsed.scheme() != "http"
        || !matches!(parsed.host_str(), Some("127.0.0.1" | "::1" | "localhost"))
        || parsed.path() != "/health"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.username() != ""
        || parsed.password().is_some()
    {
        bail!("daemon health URL must be a loopback HTTP /health endpoint");
    }
    Ok(value.to_owned())
}

pub fn daemon_health_url() -> Result<String> {
    if let Ok(configured) = env::var(DAEMON_HEALTH_URL_ENV) {
        return validate_daemon_health_url(&configured);
    }
    if let Ok(base_url) = env::var(ANTHROPIC_BASE_URL_ENV)
        && let Ok(parsed) = Url::parse(&base_url)
        && matches!(parsed.host_str(), Some("127.0.0.1" | "::1" | "localhost"))
    {
        let mut origin = parsed;
        origin.set_path("/health");
        origin.set_query(None);
        origin.set_fragment(None);
        return validate_daemon_health_url(origin.as_str());
    }
    if let Some(live) = live_generation_health_url() {
        return validate_daemon_health_url(&live);
    }
    validate_daemon_health_url(DEFAULT_DAEMON_HEALTH_URL)
}

fn live_generation_health_url() -> Option<String> {
    let home = env::var_os("HOME")?;
    let listen = env::var("CLAUDEX_ADAPTER_LISTEN").unwrap_or_else(|_| "127.0.0.1:8318".to_owned());
    let port = listen.rsplit(':').next()?;
    let path = PathBuf::from(home)
        .join(".cache/claudex")
        .join(format!("live.{port}.json"));
    let state: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let listen = state.get("listen")?.as_str()?;
    if listen.is_empty() {
        return None;
    }
    Some(format!("http://{listen}/health"))
}

pub fn sanitize_model_concurrency(value: &Value) -> Option<BTreeMap<String, Value>> {
    let object = value.as_object()?;
    let mut sanitized = BTreeMap::new();
    for (model, fields) in object {
        if !valid_model_id(&Value::from(model.as_str())) || !fields.is_object() {
            return None;
        }
        let active = fields.get("active").and_then(Value::as_i64)?;
        let queued = fields.get("queued").and_then(Value::as_i64)?;
        let limit = fields.get("limit").and_then(Value::as_i64)?;
        let available = fields.get("available").and_then(Value::as_bool)?;
        if fields.get("active").is_some_and(Value::is_boolean)
            || fields.get("queued").is_some_and(Value::is_boolean)
            || fields.get("limit").is_some_and(Value::is_boolean)
            || active < 0
            || queued < 0
            || limit <= 0
        {
            return None;
        }
        sanitized.insert(
            model.clone(),
            serde_json::json!({
                "active": active,
                "queued": queued,
                "limit": limit,
                "available": available && active + queued < limit,
            }),
        );
    }
    Some(sanitized)
}

pub fn sanitize_active_subagent_models(value: &Value) -> Option<BTreeMap<String, i64>> {
    let object = value.as_object()?;
    let mut sanitized = BTreeMap::new();
    for (model, count) in object {
        if !valid_model_id(&Value::from(model.as_str())) {
            return None;
        }
        let active = count.as_i64()?;
        if active < 0 {
            return None;
        }
        if active > 0 {
            sanitized.insert(model.clone(), active);
        }
    }
    Some(sanitized)
}

/// Live daemon capacity signals used to refresh routing between SubAgent launches.
#[derive(Clone, Debug, Default)]
pub struct DaemonHealth {
    pub model_concurrency: BTreeMap<String, Value>,
    pub active_subagent_models: BTreeMap<String, i64>,
}

pub fn run_daemon_health_before(
    curl_program: &str,
    deadline: Option<Deadline>,
) -> Option<DaemonHealth> {
    let url = daemon_health_url().ok()?;
    let mut command = Command::new(curl_program);
    command.args([
        "--fail",
        "--silent",
        "--show-error",
        "--max-time",
        &DAEMON_HEALTH_TIMEOUT_SECONDS.to_string(),
        &url,
    ]);
    let (code, stdout, _) = run_collector(
        command,
        deadline,
        Duration::from_secs(DAEMON_HEALTH_TIMEOUT_SECONDS + SUBPROCESS_GRACE_SECONDS),
    )
    .ok()?;
    if code != 0 {
        return None;
    }
    let payload: Value = serde_json::from_str(&stdout).ok()?;
    if payload.get("status").and_then(Value::as_str) != Some("ok") {
        return None;
    }
    Some(DaemonHealth {
        model_concurrency: sanitize_model_concurrency(payload.get("model_concurrency")?)?,
        active_subagent_models: payload
            .get("active_subagent_models")
            .and_then(sanitize_active_subagent_models)
            .unwrap_or_default(),
    })
}

pub fn parse_vm_stat_value(output: &str, key: &str) -> Option<i64> {
    let prefix = format!("{key}:");
    for line in output.lines() {
        let stripped = line.trim();
        if !stripped.starts_with(&prefix) {
            continue;
        }
        let token = stripped[prefix.len()..]
            .trim()
            .trim_end_matches('.')
            .split_whitespace()
            .next()?
            .replace(',', "");
        return token.parse().ok();
    }
    None
}

pub fn vm_stat_page_size(output: &str) -> Option<i64> {
    for line in output.lines() {
        let token = "page size of";
        if !line.contains(token) {
            continue;
        }
        let digits = line
            .split_once(token)?
            .1
            .split_whitespace()
            .next()?
            .replace(',', "")
            .trim_end_matches('.')
            .to_owned();
        return digits.parse().ok();
    }
    None
}

pub fn read_memory_status_before(deadline: Option<Deadline>) -> Value {
    if !memory_management_enabled() {
        return serde_json::json!({ "status": "disabled" });
    }
    let vm = (|| {
        let command = Command::new("vm_stat");
        let (code, stdout, _) = run_collector(
            command,
            deadline,
            Duration::from_secs(MEMORY_COMMAND_TIMEOUT_SECONDS),
        )?;
        if code != 0 {
            bail!("vm_stat failed");
        }
        Ok(stdout)
    })();
    let total = (|| {
        let mut command = Command::new("sysctl");
        command.args(["-n", "hw.memsize"]);
        let (code, stdout, _) = run_collector(
            command,
            deadline,
            Duration::from_secs(MEMORY_COMMAND_TIMEOUT_SECONDS),
        )?;
        if code != 0 {
            bail!("sysctl failed");
        }
        Ok(stdout)
    })();
    let (Ok(vm_output), Ok(total_memory)) = (vm, total) else {
        return serde_json::json!({ "status": "unavailable" });
    };
    let total_memory = total_memory.trim();
    if !total_memory.chars().all(|c| c.is_ascii_digit()) {
        return serde_json::json!({ "status": "unavailable" });
    }
    let page_size = vm_stat_page_size(&vm_output);
    let free_pages = parse_vm_stat_value(&vm_output, "Pages free");
    let inactive_pages = parse_vm_stat_value(&vm_output, "Pages inactive");
    let speculative_pages = parse_vm_stat_value(&vm_output, "Pages speculative");
    let (Some(page_size), Some(free_pages), Some(inactive_pages), Some(speculative_pages)) =
        (page_size, free_pages, inactive_pages, speculative_pages)
    else {
        return serde_json::json!({ "status": "unavailable" });
    };
    let total_mb = total_memory.parse::<f64>().unwrap_or_default() / (1024.0 * 1024.0);
    let available_mb = ((free_pages + inactive_pages + speculative_pages) as f64
        * page_size as f64)
        / (1024.0 * 1024.0);
    let available_percent = 100.0 * available_mb / total_mb;
    let thresholds = match memory_pressure_thresholds() {
        Ok(value) => value,
        Err(_) => return serde_json::json!({ "status": "unavailable" }),
    };
    serde_json::json!({
        "status": "available",
        "total_mb": python_round(total_mb, 1),
        "available_mb": python_round(available_mb, 1),
        "available_percent": python_round(available_percent, 1),
        "pressure_level": pressure_level(available_percent, thresholds),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_provider_models_still_participate_in_capacity_collection() {
        let config = Config {
            raw: Value::Null,
            providers: vec![
                serde_json::json!({
                    "id": "codex",
                    "usageProvider": "codex",
                    "defaultModel": "gpt-5.6-luna"
                }),
                serde_json::json!({
                    "id": "grok",
                    "usageProvider": "grok",
                    "defaultModel": "grok-4.6"
                }),
            ],
            native_workers: Vec::new(),
            fallback: Value::Null,
            advisor: Value::Null,
        };
        let disabled = BTreeSet::from(["grok-4.6".to_owned()]);

        let providers = providers_for_collection(&config, &disabled);
        let provider_ids: Vec<_> = providers
            .iter()
            .filter_map(|provider| provider.get("id").and_then(Value::as_str))
            .collect();

        assert_eq!(provider_ids, ["codex", "grok"]);
        assert_eq!(
            codexbar_usage_names(&providers, &config),
            BTreeSet::from(["codex".to_owned(), "grok".to_owned()])
        );
    }

    #[test]
    fn parses_vm_stat_values_and_page_size() {
        let output = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                               1000.\nPages inactive:                          2,000.\nPages speculative:                         50.\n";
        assert_eq!(vm_stat_page_size(output), Some(16384));
        assert_eq!(parse_vm_stat_value(output, "Pages free"), Some(1000));
        assert_eq!(parse_vm_stat_value(output, "Pages inactive"), Some(2000));
        assert_eq!(parse_vm_stat_value(output, "Pages speculative"), Some(50));
        assert_eq!(parse_vm_stat_value(output, "Pages wired down"), None);
    }

    #[test]
    fn prefers_live_generation_health_over_the_canonical_port() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join(".cache/claudex");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(
            cache.join("live.8318.json"),
            r#"{"listen":"127.0.0.1:52890","build_id":"new","pid":9}"#,
        )
        .unwrap();
        let previous_home = env::var_os("HOME");
        let previous_health = env::var_os("CLAUDEX_DAEMON_HEALTH_URL");
        let previous_base = env::var_os("ANTHROPIC_BASE_URL");
        unsafe {
            env::set_var("HOME", root.path());
            env::remove_var("CLAUDEX_DAEMON_HEALTH_URL");
            env::remove_var("ANTHROPIC_BASE_URL");
        }
        let url = daemon_health_url().unwrap();
        unsafe {
            match previous_home {
                Some(home) => env::set_var("HOME", home),
                None => env::remove_var("HOME"),
            }
            match previous_health {
                Some(value) => env::set_var("CLAUDEX_DAEMON_HEALTH_URL", value),
                None => env::remove_var("CLAUDEX_DAEMON_HEALTH_URL"),
            }
            match previous_base {
                Some(value) => env::set_var("ANTHROPIC_BASE_URL", value),
                None => env::remove_var("ANTHROPIC_BASE_URL"),
            }
        }
        assert_eq!(url, "http://127.0.0.1:52890/health");
    }

    #[test]
    fn sanitizes_daemon_model_concurrency() {
        let payload = serde_json::json!({
            "gpt-5.6-luna": {"active":1,"queued":2,"limit":5,"available":true}
        });
        let sanitized = sanitize_model_concurrency(&payload).unwrap();
        assert_eq!(sanitized["gpt-5.6-luna"]["available"], true);
        assert_eq!(sanitized["gpt-5.6-luna"]["remaining"].as_i64(), None);
        assert_eq!(sanitized["gpt-5.6-luna"]["active"], 1);
        let bad =
            serde_json::json!({"bad model": {"active":1,"queued":0,"limit":1,"available":true}});
        assert!(sanitize_model_concurrency(&bad).is_none());
    }

    #[test]
    fn embedded_oauth_helper_uses_refresh_only_bash_stdin_contract() {
        const INSTALL_LAUNCH_AGENT: &[u8] = b"--install-launch-agent";
        let (script, arguments) = oauth_contract();
        assert!(script.starts_with(b"#!/usr/bin/env bash\n"));
        assert!(
            script
                .windows(INSTALL_LAUNCH_AGENT.len())
                .any(|window| window == INSTALL_LAUNCH_AGENT)
        );
        assert_eq!(arguments, ["-s", "--", "--refresh-within", "7200"]);
    }

    #[test]
    fn reads_ollama_cloud_balance_as_codexbar_windows() {
        let payload = r#"{"included":{"session":{"remaining_percent":94.5,"resets_at":"2026-11-16T03:00:00Z"},"weekly":{"remaining_percent":99.03,"resets_at":"2026-11-23T00:00:00Z"}},"purchased":{"balance_usd":0}}"#;
        let usage = ollama_balance_windows(payload).unwrap();
        assert_eq!(usage["primary"]["usedPercent"], 5.5);
        assert_eq!(usage["primary"]["windowMinutes"], 300);
        assert_eq!(usage["secondary"]["usedPercent"], 0.97);
        assert_eq!(usage["secondary"]["windowMinutes"], 10080);
    }

    #[test]
    fn drops_ollama_balance_windows_it_cannot_read() {
        assert_eq!(
            ollama_balance_windows(r#"{"included":{"session":{"remaining_percent":120}}}"#),
            None
        );
        assert_eq!(ollama_balance_windows(r#"{"included":{}}"#), None);
        assert_eq!(
            ollama_balance_windows(r#"{"purchased":{"balance_usd":0}}"#),
            None
        );
        assert_eq!(ollama_balance_windows(""), None);
        let weekly_only =
            ollama_balance_windows(r#"{"included":{"weekly":{"remaining_percent":100}}}"#).unwrap();
        assert_eq!(weekly_only["secondary"]["usedPercent"], 0.0);
        assert!(weekly_only.get("primary").is_none());
    }

    #[test]
    fn reads_the_ollama_key_from_the_dotenv_files_herdr_app_reads() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".codex")).unwrap();
        std::fs::write(
            root.path().join(".codex/.env"),
            "OTHER=1\nOLLAMA_API_KEY=\"from-codex\"\n",
        )
        .unwrap();
        std::fs::write(root.path().join(".env"), "OLLAMA_API_KEY=from-home\n").unwrap();
        assert_eq!(dotenv_key(root.path()).as_deref(), Some("from-codex"));

        std::fs::write(root.path().join(".codex/.env"), "OLLAMA_API_KEY=\n").unwrap();
        assert_eq!(dotenv_key(root.path()).as_deref(), Some("from-home"));

        std::fs::remove_file(root.path().join(".env")).unwrap();
        assert_eq!(dotenv_key(root.path()), None);
    }

    #[test]
    fn falls_back_to_the_pi_login_for_the_ollama_key() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".pi/agent")).unwrap();
        let auth = root.path().join(".pi/agent/auth.json");
        std::fs::write(
            &auth,
            r#"{"ollama-cloud":{"key":"from-pi","type":"api"},"xai":{"key":"other"}}"#,
        )
        .unwrap();
        assert_eq!(
            pi_provider_key(root.path(), "ollama-cloud").as_deref(),
            Some("from-pi")
        );
        assert_eq!(pi_provider_key(root.path(), "opencode-go"), None);

        std::fs::write(&auth, r#"{"xai":{"key":"other"}}"#).unwrap();
        assert_eq!(pi_provider_key(root.path(), "ollama-cloud"), None);

        std::fs::write(&auth, "not json").unwrap();
        assert_eq!(pi_provider_key(root.path(), "ollama-cloud"), None);
    }
}
