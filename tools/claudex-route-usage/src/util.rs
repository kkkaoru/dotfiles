//! Small shared helpers: env parsing, rounding, cache IO, and time formatting.

use anyhow::{Result, bail};
use chrono::{SecondsFormat, TimeZone, Utc};
use serde_json::{Map, Value};
use std::io::Write as _;
use std::path::Path;

pub const DEFAULT_CACHE_SECONDS: i64 = 300;

/// Parse the optional cache TTL, falling back safely on invalid values.
pub fn cache_seconds() -> i64 {
    match std::env::var("CLAUDEX_USAGE_CACHE_SECONDS") {
        Ok(raw) => match raw.trim().parse::<i64>() {
            Ok(value) => value.max(0),
            Err(_) => DEFAULT_CACHE_SECONDS,
        },
        Err(_) => DEFAULT_CACHE_SECONDS,
    }
}

/// Parse a strict boolean orchestration switch (matches `_boolean_or_default`).
pub fn boolean_env(name: &str, default: bool) -> Result<bool> {
    match std::env::var(name) {
        Err(_) => Ok(default),
        Ok(raw) => boolean_value(&raw, name, default),
    }
}

pub fn boolean_value(raw: &str, name: &str, default: bool) -> Result<bool> {
    if raw.trim().is_empty() {
        return Ok(default);
    }
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => bail!("{name} must be one of 0, 1, true, or false"),
    }
}

/// Parse one positive orchestration integer and reject unsafe values.
pub fn positive_or_default(raw: Option<&str>, name: &str, default: i64, minimum: i64) -> Result<i64> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    if raw.trim().is_empty() {
        return Ok(default);
    }
    let value = raw
        .trim()
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!("{name} must be an integer >= {minimum}"))?;
    if value < minimum {
        bail!("{name} must be an integer >= {minimum}");
    }
    Ok(value)
}

/// Accept a finite, non-negative percentage while rejecting JSON booleans.
pub fn valid_percentage(value: &Value) -> bool {
    if value.is_boolean() {
        return false;
    }
    value.as_f64().is_some_and(|number| number.is_finite() && number >= 0.0)
}

/// A JSON number as f64 (booleans are never numbers in JSON).
pub fn number_f64(value: &Value) -> Option<f64> {
    if value.is_boolean() {
        return None;
    }
    value.as_f64()
}

/// Python-style rounding to `ndigits` decimals for realistic quota values.
pub fn python_round(value: f64, ndigits: i32) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let factor = 10f64.powi(ndigits);
    (value * factor).round() / factor
}

pub fn model_family(model: &str) -> String {
    let mut current = model;
    for separator in ['/', '-', '_', '.'] {
        current = current.split(separator).next().unwrap_or(current);
    }
    current.to_owned()
}

pub fn is_sonnet_model(model: Option<&str>) -> bool {
    let Some(model) = model else {
        return false;
    };
    matches!(
        model.trim().to_ascii_lowercase().as_str(),
        "sonnet" | "sonnet[1m]" | "claude-sonnet-5" | "claude-sonnet-5[1m]"
    )
}

/// Format a Unix timestamp as an explicit UTC ISO-8601 acquisition time.
pub fn format_utc_datetime(timestamp: f64) -> String {
    let seconds = timestamp.trunc() as i64;
    let nanos = ((timestamp - timestamp.trunc()) * 1_000_000_000.0).round() as u32;
    Utc.timestamp_opt(seconds, nanos)
        .single()
        .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Micros, true))
        .unwrap_or_default()
}

/// Parse the UTC ISO-8601 acquisition time stored in the quota cache.
pub fn parse_utc_datetime(value: &Value) -> Result<f64> {
    let Some(text) = value.as_str() else {
        bail!("Qwen quota cache has an invalid acquisition time");
    };
    let Some(stripped) = text.strip_suffix('Z') else {
        bail!("Qwen quota cache has an invalid acquisition time");
    };
    let parsed = chrono::DateTime::parse_from_rfc3339(&format!("{stripped}+00:00"))
        .map_err(|_| anyhow::anyhow!("Qwen quota cache has an invalid acquisition time"))?;
    let seconds = parsed.timestamp() as f64;
    let subsec = f64::from(parsed.timestamp_subsec_nanos()) / 1_000_000_000.0;
    Ok(seconds + subsec)
}

/// Read a fresh, already-sanitized routing summary for this config.
pub fn read_routing_cache(path: &Path, now: f64, ttl: i64, expected_key: &str) -> Option<Value> {
    if ttl <= 0 {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let cached: Value = serde_json::from_str(&text).ok()?;
    if cached.get("configuration_key").and_then(Value::as_str) != Some(expected_key) {
        return None;
    }
    let created_at = number_f64(cached.get("created_at")?)?;
    if now - created_at <= ttl as f64 {
        cached.get("summary").cloned()
    } else {
        None
    }
}

/// Atomically cache only the sanitized summary, never raw Codexbar output.
pub fn write_routing_cache(path: &Path, summary: &Value, now: f64, key: &str) -> Result<()> {
    let mut object = Map::new();
    object.insert("created_at".into(), Value::from(now));
    object.insert("configuration_key".into(), Value::from(key));
    object.insert("summary".into(), summary.clone());
    write_private_json(path, &Value::Object(object))
}

/// Atomically write private JSON with owner-only permissions.
pub fn write_private_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_string(value)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temp = tempfile::Builder::new().tempfile_in(parent)?;
    temp.write_all(payload.as_bytes())?;
    temp.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o600))?;
    }
    temp.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_family_splits_on_first_separator() {
        assert_eq!(model_family("gpt-5.6-luna"), "gpt");
        assert_eq!(model_family("opencode-go/deepseek"), "opencode");
        assert_eq!(model_family("claude-sonnet-5"), "claude");
    }

    #[test]
    fn sonnet_aliases_are_recognized() {
        assert!(is_sonnet_model(Some("claude-sonnet-5")));
        assert!(is_sonnet_model(Some("sonnet[1m]")));
        assert!(!is_sonnet_model(Some("gpt-5.6-luna")));
        assert!(!is_sonnet_model(None));
    }

    #[test]
    fn boolean_env_parsing_is_strict() {
        assert!(boolean_value("on", "X", false).unwrap());
        assert!(!boolean_value("off", "X", true).unwrap());
        assert!(boolean_value("", "X", true).unwrap());
        assert!(boolean_value("maybe", "X", false).is_err());
    }
}
