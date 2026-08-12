use serde_json::Value;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const CHILD_SKIP_ENVS: &[&str] = &[
    "CLAUDEX_NONINTERACTIVE_CHILD",
    "CLAUDEX_GROK_ACP",
    "CLAUDEX_PROVIDER_ACP",
];

pub(crate) fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub(crate) fn cache_dir() -> PathBuf {
    if let Ok(override_dir) = env::var("CLAUDEX_CACHE_DIR")
        && !override_dir.trim().is_empty()
    {
        return PathBuf::from(override_dir);
    }
    home_dir().join(".cache").join("claudex")
}

pub(crate) fn env_truthy(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(raw) if !raw.trim().is_empty() => {
            matches!(
                raw.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        }
        _ => default,
    }
}

pub(crate) fn is_child_runtime() -> bool {
    CHILD_SKIP_ENVS.iter().any(|name| env_truthy(name, false))
}

pub(crate) fn load_json_object(path: &std::path::Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    if value.is_object() { Some(value) } else { None }
}

pub(crate) fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub(crate) fn nonempty_str(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}
