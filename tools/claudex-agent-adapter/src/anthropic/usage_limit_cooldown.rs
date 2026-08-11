use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

const CACHE_FILE_NAME: &str = "codex-app-server-usage-limit.json";
const CACHE_VERSION: u8 = 1;
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(4 * 60 * 60);
const MAX_COOLDOWN: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const COOLDOWN_ENV: &str = "CLAUDEX_CODEX_USAGE_LIMIT_COOLDOWN_SECONDS";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageLimitCooldown {
    version: u8,
    backend: String,
    until_unix_seconds: u64,
    message: String,
    recorded_unix_seconds: u64,
}

impl UsageLimitCooldown {
    pub(crate) fn is_active(&self, now: SystemTime) -> bool {
        unix_seconds(now) < self.until_unix_seconds
    }
}

pub(crate) fn cache_path_for_home(home: impl AsRef<Path>) -> PathBuf {
    home.as_ref().join(".cache/claudex").join(CACHE_FILE_NAME)
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn current_cache_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(cache_path_for_home)
}

pub(crate) fn load_active(path: &Path, now: SystemTime) -> Option<UsageLimitCooldown> {
    let bytes = fs::read(path).ok()?;
    let stored: UsageLimitCooldown = serde_json::from_slice(&bytes).ok()?;
    if stored.version != CACHE_VERSION {
        return None;
    }
    stored.is_active(now).then_some(stored)
}

pub(crate) fn record_codex_app_server_limit_at(
    path: Option<&Path>,
    message: &str,
    now: SystemTime,
) -> Option<PathBuf> {
    let path = path?;
    let cooldown = UsageLimitCooldown {
        version: CACHE_VERSION,
        backend: crate::agent_backend::BackendKind::CodexAppServer
            .as_str()
            .to_owned(),
        until_unix_seconds: unix_seconds(now + cooldown_duration(message, now)),
        message: message.to_owned(),
        recorded_unix_seconds: unix_seconds(now),
    };
    write_cooldown(path, &cooldown);
    Some(path.to_path_buf())
}

pub(crate) fn codex_app_server_is_cooling_down_at(path: Option<&Path>, now: SystemTime) -> bool {
    path.and_then(|path| load_active(path, now))
        .is_some_and(|cooldown| {
            cooldown.backend == crate::agent_backend::BackendKind::CodexAppServer.as_str()
        })
}

fn cooldown_duration(message: &str, _now: SystemTime) -> Duration {
    if let Ok(seconds) = std::env::var(COOLDOWN_ENV)
        && let Ok(seconds) = seconds.parse::<u64>()
    {
        return Duration::from_secs(seconds.min(MAX_COOLDOWN.as_secs()));
    }
    // Prefer the configured/default window. Exact clock parsing needs local TZ
    // support the adapter deliberately avoids depending on; re-probe after expiry.
    let _ = message;
    DEFAULT_COOLDOWN.min(MAX_COOLDOWN)
}

fn write_cooldown(path: &Path, cooldown: &UsageLimitCooldown) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(payload) = serde_json::to_vec_pretty(cooldown) else {
        return;
    };
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let wrote = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(&payload)?;
            file.sync_all()
        });
    if wrote.is_ok() {
        let _ = fs::rename(&temporary, path);
    }
    let _ = fs::remove_file(&temporary);
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
include!("usage_limit_cooldown_tests.rs");

