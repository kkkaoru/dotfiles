//! Provider usage collection, Qwen/Ollama probes, daemon health, and memory.

use crate::config::{Config, Paths, valid_model_id};
use crate::routing::{
    explicitly_reported_status, memory_management_enabled, memory_pressure_thresholds,
    pressure_level, provider_status, worker, FIVE_HOUR_WINDOW, SEVEN_DAY_WINDOW,
};
use crate::util::{format_utc_datetime, number_f64, parse_utc_datetime, python_round, valid_percentage, write_private_json};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use url::Url;

pub const QWEN_QUOTA_CACHE_SECONDS: i64 = 60 * 60;
pub const QWEN_REQUEST_TIMEOUT_SECONDS: u64 = 5;
pub const QWEN_SUBPROCESS_GRACE_SECONDS: u64 = 2;
pub const USAGE_COMMAND_TIMEOUT_SECONDS: u64 = 45;
pub const DAEMON_HEALTH_TIMEOUT_SECONDS: u64 = 2;
pub const MEMORY_COMMAND_TIMEOUT_SECONDS: u64 = 5;
pub const QWEN_USAGE_PROVIDER: &str = "qwen";
pub const OLLAMA_USAGE_PROVIDER: &str = "ollama";
pub const OLLAMA_BASE_URL_ENV: &str = "CLAUDEX_OLLAMA_BASE_URL";
pub const DEFAULT_OLLAMA_BASE_URL: &str = "http://127.0.0.1:11434";
pub const DAEMON_HEALTH_URL_ENV: &str = "CLAUDEX_DAEMON_HEALTH_URL";
pub const ANTHROPIC_BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";
pub const DEFAULT_DAEMON_HEALTH_URL: &str = "http://127.0.0.1:8318/health";
pub const QWEN_CONSOLE_HOST: &str = "cs-data.qwencloud.com";
pub const QWEN_CONSOLE_PATH: &str = "/data/api.json";
pub const QWEN_CONSOLE_PRODUCT: &str = "sfm_bailian";
pub const QWEN_CONSOLE_ACTION: &str = "IntlBroadScopeAspnGateway";
pub const QWEN_QUOTA_API: &str = "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/usage";

fn run_with_timeout(mut command: Command, timeout: Duration) -> Result<(i32, String, String)> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().context("failed to spawn subprocess")?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let out = stdout
            .map(|mut stream| {
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(&mut stream, &mut buf);
                buf
            })
            .unwrap_or_default();
        let err = stderr
            .map(|mut stream| {
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(&mut stream, &mut buf);
                buf
            })
            .unwrap_or_default();
        let status = child.wait();
        let _ = tx.send((status, out, err));
    });
    match rx.recv_timeout(timeout) {
        Ok((status, out, err)) => {
            let code = status.map(|s| s.code().unwrap_or(1)).unwrap_or(1);
            Ok((code, out, err))
        }
        Err(_) => bail!("subprocess timed out"),
    }
}

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

pub fn run_codexbar(program: &str) -> Result<Value> {
    let mut command = Command::new(program);
    command.args(["usage", "--json"]);
    let (_code, stdout, _stderr) = run_with_timeout(
        command,
        Duration::from_secs(USAGE_COMMAND_TIMEOUT_SECONDS),
    )?;
    strict_json_array(&stdout)
}

fn single_value(values: &BTreeMap<String, Vec<String>>, key: &str) -> Result<String> {
    let matches = values.get(key).cloned().unwrap_or_default();
    if matches.len() != 1 || matches[0].is_empty() {
        bail!("Qwen request must contain one {key}");
    }
    Ok(matches[0].clone())
}

fn next_curl_value(tokens: &[String], index: usize) -> Result<&str> {
    tokens
        .get(index + 1)
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!("Qwen curl option is missing a value"))
}

fn unique_curl_value(current: &str, value: &str, name: &str) -> Result<String> {
    if !current.is_empty() || value.is_empty() {
        bail!("Qwen curl command has an invalid {name}");
    }
    Ok(value.to_owned())
}

fn parse_qs(raw: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode(key);
        let value = percent_decode(value);
        out.entry(key).or_default().push(value);
    }
    out
}

fn percent_decode(value: &str) -> String {
    let mut out = Vec::new();
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &value[i + 1..i + 3];
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn validate_qwen_request(url: &str, cookie: &str, body: &str, content_type: &str) -> Result<()> {
    let parsed = Url::parse(url).context("Qwen curl command targets an unexpected endpoint")?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some(QWEN_CONSOLE_HOST)
        || parsed.path() != QWEN_CONSOLE_PATH
        || !(parsed.port().is_none() || parsed.port() == Some(443))
        || parsed.username() != ""
        || parsed.fragment().is_some()
        || url.chars().any(|c| matches!(c, '\r' | '\n' | '\0'))
        || cookie.chars().any(|c| matches!(c, '\r' | '\n' | '\0'))
        || body.chars().any(|c| matches!(c, '\r' | '\n' | '\0'))
    {
        bail!("Qwen curl command targets an unexpected endpoint");
    }
    let query = parse_qs(parsed.query().unwrap_or_default());
    let keys: BTreeSet<&str> = query.keys().map(String::as_str).collect();
    if keys != BTreeSet::from(["product", "action", "api"]) {
        bail!("Qwen curl command has unexpected query fields");
    }
    if single_value(&query, "product")? != QWEN_CONSOLE_PRODUCT
        || single_value(&query, "action")? != QWEN_CONSOLE_ACTION
        || single_value(&query, "api")? != QWEN_QUOTA_API
    {
        bail!("Qwen curl command targets an unexpected API");
    }
    let form = parse_qs(body);
    let form_keys: BTreeSet<&str> = form.keys().map(String::as_str).collect();
    if form_keys != BTreeSet::from(["product", "action", "sec_token", "region", "params"]) {
        bail!("Qwen curl command has unexpected form fields");
    }
    let parameters: Value = serde_json::from_str(&single_value(&form, "params")?)?;
    if single_value(&form, "product")? != QWEN_CONSOLE_PRODUCT
        || single_value(&form, "action")? != QWEN_CONSOLE_ACTION
        || single_value(&form, "sec_token")?.is_empty()
        || single_value(&form, "region")? != "ap-southeast-1"
        || !parameters.is_object()
        || parameters.get("Api").and_then(Value::as_str) != Some(QWEN_QUOTA_API)
        || !parameters.get("Data").map(Value::is_object).unwrap_or(false)
        || parameters.get("V").and_then(Value::as_str) != Some("1.0")
        || !cookie.contains('=')
        || content_type != "application/x-www-form-urlencoded"
    {
        bail!("Qwen curl command contains invalid request data");
    }
    Ok(())
}

pub fn qwen_curl_request(path: &Path) -> Result<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(path)?;
    let tokens: Vec<String> = shell_words::split(&text)?
        .into_iter()
        .filter(|token| !token.trim().is_empty() && token != "\\")
        .collect();
    if tokens
        .first()
        .map(|token| Path::new(token).file_name().and_then(|name| name.to_str()) != Some("curl"))
        .unwrap_or(true)
    {
        bail!("Qwen quota input must be a curl command");
    }
    let mut url = String::new();
    let mut cookie = String::new();
    let mut body = String::new();
    let mut content_type = String::new();
    let mut index = 1usize;
    while index < tokens.len() {
        let token = &tokens[index];
        if token.starts_with("https://") {
            if !url.is_empty() {
                bail!("Qwen curl command must contain one URL");
            }
            url = token.clone();
            index += 1;
            continue;
        }
        let (option, separator, inline) = if let Some((left, right)) = token.split_once('=') {
            (left, true, right.to_owned())
        } else {
            (token.as_str(), false, String::new())
        };
        if matches!(
            option,
            "-H" | "--header" | "-b" | "--cookie" | "--data" | "--data-raw"
        ) {
            let value = if separator {
                inline
            } else {
                next_curl_value(&tokens, index)?.to_owned()
            };
            index += if separator { 1 } else { 2 };
            if matches!(option, "-H" | "--header") {
                if let Some((name, header_value)) = value.split_once(':') {
                    if name.trim().eq_ignore_ascii_case("content-type") {
                        content_type = header_value.trim().to_ascii_lowercase();
                    }
                }
            } else if matches!(option, "-b" | "--cookie") {
                cookie = unique_curl_value(&cookie, &value, "cookie")?;
            } else {
                body = unique_curl_value(&body, &value, "request body")?;
            }
            continue;
        }
        bail!("Qwen curl command contains an unsupported argument");
    }
    validate_qwen_request(&url, &cookie, &body, &content_type)?;
    Ok(BTreeMap::from([
        ("url".into(), url),
        ("cookie".into(), cookie),
        ("body".into(), body),
    ]))
}

fn quota_fraction(value: &Value, name: &str) -> Result<f64> {
    if !valid_percentage(value) || number_f64(value).is_some_and(|n| n > 1.0) {
        bail!("Qwen quota response contains invalid {name}");
    }
    number_f64(value).ok_or_else(|| anyhow::anyhow!("Qwen quota response contains invalid {name}"))
}

fn quota_reset(value: &Value, name: &str) -> Result<i64> {
    let Some(number) = number_f64(value) else {
        bail!("Qwen quota response contains invalid {name}");
    };
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 {
        bail!("Qwen quota response contains invalid {name}");
    }
    Ok(number as i64)
}

pub fn qwen_quota_window(quota: &Value, name: &str, percentage_key: &str, reset_key: &str) -> Result<Value> {
    let used = python_round(quota_fraction(&quota[percentage_key], percentage_key)? * 100.0, 6);
    Ok(serde_json::json!({
        "name": name,
        "usedPercent": used,
        "remainingPercent": python_round(100.0 - used, 6),
        "resetAtMilliseconds": quota_reset(&quota[reset_key], reset_key)?,
    }))
}

pub fn qwen_quota_entry(payload: &Value, provider: &str) -> Result<Value> {
    let quota = payload
        .pointer("/data/DataV2/data/data")
        .ok_or_else(|| anyhow::anyhow!("Qwen quota response is missing usage data"))?;
    if !quota.is_object() {
        bail!("Qwen quota response usage data must be an object");
    }
    let windows = vec![
        qwen_quota_window(quota, FIVE_HOUR_WINDOW, "per5HourPercentage", "per5HourResetTime")?,
        qwen_quota_window(quota, SEVEN_DAY_WINDOW, "per1WeekPercentage", "per1WeekResetTime")?,
    ];
    let maximum = windows
        .iter()
        .filter_map(|window| number_f64(&window["usedPercent"]))
        .fold(f64::NEG_INFINITY, f64::max);
    let available = maximum < 100.0;
    Ok(serde_json::json!({
        "provider": provider,
        "available": available,
        "reason": if available { "available-qwen-cloud-quota" } else { "exhausted" },
        "maxUsedPercent": maximum,
        "quotaWindows": windows,
    }))
}

pub fn run_qwen_quota(program: &str, path: &Path, provider: &str) -> Result<Value> {
    let request = qwen_curl_request(path)?;
    let mut command = Command::new(program);
    command.args([
        "--silent",
        "--show-error",
        "--fail-with-body",
        "--max-time",
        &QWEN_REQUEST_TIMEOUT_SECONDS.to_string(),
        &request["url"],
        "--header",
        "accept: application/json",
        "--header",
        "content-type: application/x-www-form-urlencoded",
        "--header",
        "origin: https://home.qwencloud.com",
        "--header",
        "referer: https://home.qwencloud.com/billing/subscription/token-plan-individual",
        "--cookie",
        &request["cookie"],
        "--data-raw",
        &request["body"],
    ]);
    let (code, stdout, stderr) = run_with_timeout(
        command,
        Duration::from_secs(QWEN_REQUEST_TIMEOUT_SECONDS + QWEN_SUBPROCESS_GRACE_SECONDS),
    )?;
    if code != 0 {
        bail!("qwen quota curl failed: {stderr}");
    }
    qwen_quota_entry(&serde_json::from_str(&stdout)?, provider)
}

pub fn qwen_quota_cache_entry(path: &Path, now: f64) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    let cached: Value = serde_json::from_str(&text).ok()?;
    let fetched_at = parse_utc_datetime(cached.get("fetched_at")?).ok()?;
    let entry = cached.get("entry")?.clone();
    let age = now - fetched_at;
    if !(0.0..QWEN_QUOTA_CACHE_SECONDS as f64).contains(&age) {
        return None;
    }
    if entry.get("provider").and_then(Value::as_str) != Some(QWEN_USAGE_PROVIDER) {
        return None;
    }
    if !matches!(
        entry.get("reason").and_then(Value::as_str),
        Some("available-qwen-cloud-quota" | "exhausted")
    ) {
        return None;
    }
    if !entry.get("quotaWindows").map(Value::is_array).unwrap_or(false) {
        return None;
    }
    if explicitly_reported_status(&entry)
        .get("reason")
        .and_then(Value::as_str)
        == Some("unknown")
    {
        return None;
    }
    Some(entry)
}

pub fn write_qwen_quota_cache(path: &Path, entry: &Value, now: f64) -> Result<()> {
    write_private_json(
        path,
        &serde_json::json!({
            "fetched_at": format_utc_datetime(now),
            "entry": entry,
        }),
    )
}

pub fn qwen_quota_refresh_due(summary: &Value, config: &Config, cache_path: &Path, now: f64) -> bool {
    let Some(providers) = summary.get("providers").and_then(Value::as_object) else {
        return false;
    };
    let qwen_ids: BTreeSet<&str> = config
        .providers
        .iter()
        .filter(|provider| {
            provider
                .get("usageProvider")
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case(QWEN_USAGE_PROVIDER))
        })
        .filter_map(|provider| provider.get("id").and_then(Value::as_str))
        .collect();
    let uses_quota = qwen_ids.iter().any(|provider_id| {
        providers.get(*provider_id).is_some_and(|fields| {
            matches!(
                fields.get("reason").and_then(Value::as_str),
                Some("available-qwen-cloud-quota" | "exhausted")
            )
        })
    });
    uses_quota && qwen_quota_cache_entry(cache_path, now).is_none()
}

pub fn qwen_compatible_configuration(path: &Path, model: &str) -> Result<(String, String)> {
    let settings: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let providers = settings
        .get("modelProviders")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("Qwen settings are missing model providers or environment"))?;
    let environment = settings
        .get("env")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("Qwen settings are missing model providers or environment"))?;
    let mut candidates = Vec::new();
    for items in providers.values() {
        let Some(items) = items.as_array() else {
            continue;
        };
        for item in items {
            if item.get("id").and_then(Value::as_str) == Some(model) {
                candidates.push(item);
            }
        }
    }
    if candidates.len() != 1 {
        bail!("Qwen settings must contain one configured model");
    }
    let candidate = candidates[0];
    let base_url = candidate
        .get("baseUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Qwen settings are missing compatible API credentials"))?;
    let environment_key = candidate.get("envKey").and_then(Value::as_str);
    let api_key = environment_key
        .and_then(|key| environment.get(key))
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Qwen settings are missing compatible API credentials"))?;
    let parsed = Url::parse(base_url)?;
    let host = parsed.host_str().unwrap_or_default();
    if parsed.scheme() != "https"
        || !host.ends_with(".maas.aliyuncs.com")
        || !host.starts_with("token-plan.")
        || parsed.path().trim_end_matches('/') != "/compatible-mode/v1"
        || !(parsed.port().is_none() || parsed.port() == Some(443))
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.username() != ""
    {
        bail!("Qwen settings contain an unexpected compatible endpoint");
    }
    Ok((format!("{}/models", base_url.trim_end_matches('/')), api_key.to_owned()))
}

pub fn qwen_compatible_available(program: &str, settings: &Path, model: &str) -> Result<bool> {
    let (endpoint, api_key) = qwen_compatible_configuration(settings, model)?;
    let mut command = Command::new(program);
    command.args([
        "--silent",
        "--show-error",
        "--fail",
        "--output",
        "/dev/null",
        "--max-time",
        &QWEN_REQUEST_TIMEOUT_SECONDS.to_string(),
        "--header",
        &format!("Authorization: Bearer {api_key}"),
        &endpoint,
    ]);
    let (code, _stdout, stderr) = run_with_timeout(
        command,
        Duration::from_secs(QWEN_REQUEST_TIMEOUT_SECONDS + QWEN_SUBPROCESS_GRACE_SECONDS),
    )?;
    if code != 0 {
        bail!("compatible api probe failed: {stderr}");
    }
    Ok(true)
}

pub fn qwen_usage_entry(
    program: &str,
    provider: &str,
    model: &str,
    curl_path: &Path,
    settings_path: &Path,
    cache_path: &Path,
    now: f64,
) -> Value {
    if let Some(cached) = qwen_quota_cache_entry(cache_path, now) {
        return cached;
    }
    match run_qwen_quota(program, curl_path, provider) {
        Ok(entry) => {
            let _ = write_qwen_quota_cache(cache_path, &entry, now);
            entry
        }
        Err(_) => match qwen_compatible_available(program, settings_path, model) {
            Ok(true) => serde_json::json!({
                "provider": provider,
                "available": true,
                "reason": "available-compatible-api-only",
            }),
            _ => unavailable_usage_entry(provider),
        },
    }
}

pub fn ollama_usage_entry(curl_program: &str, provider: &str, model: &str) -> Value {
    let base_url = env::var(OLLAMA_BASE_URL_ENV).unwrap_or_else(|_| DEFAULT_OLLAMA_BASE_URL.to_owned());
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
        &QWEN_REQUEST_TIMEOUT_SECONDS.to_string(),
        &format!("{base_url}/api/tags"),
    ]);
    let Ok((code, stdout, _)) = run_with_timeout(
        command,
        Duration::from_secs(QWEN_REQUEST_TIMEOUT_SECONDS + QWEN_SUBPROCESS_GRACE_SECONDS),
    ) else {
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
    serde_json::json!({
        "provider": provider,
        "available": true,
        "reason": "available-ollama-api-only",
    })
}

pub fn collect_codexbar_report(
    codexbar_program: &str,
    codexbar_names: &BTreeSet<String>,
    qwen_names: &BTreeSet<String>,
) -> Vec<Value> {
    match run_codexbar(codexbar_program) {
        Ok(Value::Array(entries)) => entries
            .into_iter()
            .filter(|entry| {
                entry
                    .get("provider")
                    .and_then(Value::as_str)
                    .map(|name| !qwen_names.contains(&name.to_ascii_lowercase()))
                    .unwrap_or(true)
            })
            .collect(),
        _ => codexbar_names
            .iter()
            .map(|name| unavailable_usage_entry(name))
            .collect(),
    }
}

pub fn collect_usage(
    config: &Config,
    codexbar_program: &str,
    curl_program: &str,
    paths: &Paths,
    now: f64,
    disabled_models: &BTreeSet<String>,
) -> Vec<Value> {
    let curl_path = env::var("CLAUDEX_QWEN_QUOTA_CURL_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| paths.repository_root.join("tmp/curl.txt"));
    let settings_path = env::var("CLAUDEX_QWEN_SETTINGS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| paths.home.join(".qwen/settings.json"));
    let quota_cache_path = paths.home.join(".cache/claudex/qwen-quota.json");
    let providers: Vec<&Value> = config
        .providers
        .iter()
        .filter(|provider| {
            worker(provider)
                .get("model")
                .and_then(Value::as_str)
                .is_some_and(|model| !disabled_models.contains(model))
        })
        .collect();
    if providers.is_empty() {
        return config
            .providers
            .iter()
            .filter_map(|provider| provider.get("id").and_then(Value::as_str))
            .map(unavailable_usage_entry)
            .collect();
    }
    let qwen_providers: Vec<&Value> = providers
        .iter()
        .copied()
        .filter(|provider| {
            provider
                .get("usageProvider")
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case(QWEN_USAGE_PROVIDER))
        })
        .collect();
    let qwen_names: BTreeSet<String> = qwen_providers
        .iter()
        .filter_map(|provider| provider.get("usageProvider").and_then(Value::as_str))
        .map(str::to_ascii_lowercase)
        .collect();
    let ollama_providers: Vec<&Value> = providers
        .iter()
        .copied()
        .filter(|provider| {
            provider
                .get("usageProvider")
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case(OLLAMA_USAGE_PROVIDER))
        })
        .collect();
    let codexbar_names: BTreeSet<String> = providers
        .iter()
        .filter(|provider| !qwen_providers.contains(*provider))
        .filter_map(|provider| provider.get("usageProvider").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();

    let codexbar_program = codexbar_program.to_owned();
    let curl_program_owned = curl_program.to_owned();
    let curl_path_owned = curl_path.clone();
    let settings_path_owned = settings_path.clone();
    let quota_cache_path_owned = quota_cache_path.clone();
    let qwen_names_for_thread = qwen_names.clone();
    let codexbar_names_for_thread = codexbar_names.clone();

    let (tx_codex, rx_codex) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx_codex.send(collect_codexbar_report(
            &codexbar_program,
            &codexbar_names_for_thread,
            &qwen_names_for_thread,
        ));
    });

    let mut qwen_handles = Vec::new();
    for provider in &qwen_providers {
        let usage_provider = provider
            .get("usageProvider")
            .and_then(Value::as_str)
            .unwrap_or(QWEN_USAGE_PROVIDER)
            .to_owned();
        let model = provider
            .get("defaultModel")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let curl_program = curl_program_owned.clone();
        let curl_path = curl_path_owned.clone();
        let settings_path = settings_path_owned.clone();
        let quota_cache_path = quota_cache_path_owned.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(qwen_usage_entry(
                &curl_program,
                &usage_provider,
                &model,
                &curl_path,
                &settings_path,
                &quota_cache_path,
                now,
            ));
        });
        qwen_handles.push(rx);
    }

    let mut report = rx_codex.recv().unwrap_or_default();
    for handle in qwen_handles {
        if let Ok(entry) = handle.recv() {
            report.push(entry);
        }
    }

    let fallback_providers: Vec<&Value> = ollama_providers
        .into_iter()
        .filter(|provider| {
            let usage_provider = provider
                .get("usageProvider")
                .and_then(Value::as_str)
                .unwrap_or_default();
            matches!(
                provider_status(&Value::Array(report.clone()), usage_provider)
                    .get("reason")
                    .and_then(Value::as_str),
                Some("missing" | "unknown" | "usage-unavailable")
            )
        })
        .collect();
    if fallback_providers.is_empty() {
        return report;
    }
    let mut handles = Vec::new();
    for provider in &fallback_providers {
        let usage_provider = provider
            .get("usageProvider")
            .and_then(Value::as_str)
            .unwrap_or(OLLAMA_USAGE_PROVIDER)
            .to_owned();
        let model = worker(provider)
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let curl_program = curl_program.to_owned();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send((
                usage_provider.clone(),
                ollama_usage_entry(&curl_program, &usage_provider, &model),
            ));
        });
        handles.push(rx);
    }
    for handle in handles {
        if let Ok((usage_provider, entry)) = handle.recv() {
            report.retain(|item| {
                item.get("provider")
                    .and_then(Value::as_str)
                    .map(|name| !name.eq_ignore_ascii_case(&usage_provider))
                    .unwrap_or(true)
            });
            report.push(entry);
        }
    }
    report
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
    if let Ok(base_url) = env::var(ANTHROPIC_BASE_URL_ENV) {
        if let Ok(parsed) = Url::parse(&base_url) {
            if matches!(parsed.host_str(), Some("127.0.0.1" | "::1" | "localhost")) {
                let mut origin = parsed;
                origin.set_path("/health");
                origin.set_query(None);
                origin.set_fragment(None);
                return validate_daemon_health_url(origin.as_str());
            }
        }
    }
    validate_daemon_health_url(DEFAULT_DAEMON_HEALTH_URL)
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

pub fn run_daemon_health(curl_program: &str) -> Option<BTreeMap<String, Value>> {
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
    let (code, stdout, _) = run_with_timeout(
        command,
        Duration::from_secs(DAEMON_HEALTH_TIMEOUT_SECONDS + QWEN_SUBPROCESS_GRACE_SECONDS),
    )
    .ok()?;
    if code != 0 {
        return None;
    }
    let payload: Value = serde_json::from_str(&stdout).ok()?;
    if payload.get("status").and_then(Value::as_str) != Some("ok") {
        return None;
    }
    sanitize_model_concurrency(payload.get("model_concurrency")?)
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
            .trim()
            .split_whitespace()
            .next()?
            .replace(',', "")
            .trim_end_matches('.')
            .to_owned();
        return digits.parse().ok();
    }
    None
}

pub fn read_memory_status() -> Value {
    if !memory_management_enabled() {
        return serde_json::json!({ "status": "disabled" });
    }
    let vm = (|| {
        let command = Command::new("vm_stat");
        let (code, stdout, _) =
            run_with_timeout(command, Duration::from_secs(MEMORY_COMMAND_TIMEOUT_SECONDS))?;
        if code != 0 {
            bail!("vm_stat failed");
        }
        Ok(stdout)
    })();
    let total = (|| {
        let mut command = Command::new("sysctl");
        command.args(["-n", "hw.memsize"]);
        let (code, stdout, _) =
            run_with_timeout(command, Duration::from_secs(MEMORY_COMMAND_TIMEOUT_SECONDS))?;
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
    let available_mb =
        ((free_pages + inactive_pages + speculative_pages) as f64 * page_size as f64) / (1024.0 * 1024.0);
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
    fn parses_vm_stat_values_and_page_size() {
        let output = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                               1000.\nPages inactive:                          2,000.\nPages speculative:                         50.\n";
        assert_eq!(vm_stat_page_size(output), Some(16384));
        assert_eq!(parse_vm_stat_value(output, "Pages free"), Some(1000));
        assert_eq!(parse_vm_stat_value(output, "Pages inactive"), Some(2000));
        assert_eq!(parse_vm_stat_value(output, "Pages speculative"), Some(50));
        assert_eq!(parse_vm_stat_value(output, "Pages wired down"), None);
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
        let bad = serde_json::json!({"bad model": {"active":1,"queued":0,"limit":1,"available":true}});
        assert!(sanitize_model_concurrency(&bad).is_none());
    }
}
