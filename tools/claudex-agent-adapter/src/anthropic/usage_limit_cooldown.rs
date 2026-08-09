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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn round_trips_an_active_cooldown() {
        let root = tempfile::tempdir().expect("cooldown fixture");
        let path = cache_path_for_home(root.path());
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let cooldown = UsageLimitCooldown {
            version: CACHE_VERSION,
            backend: "codex-app-server".to_owned(),
            until_unix_seconds: 1_000 + 60,
            message: "You've hit your usage limit.".to_owned(),
            recorded_unix_seconds: 1_000,
        };
        write_cooldown(&path, &cooldown);
        assert_eq!(load_active(&path, now).as_ref(), Some(&cooldown));
        assert!(load_active(&path, now + Duration::from_secs(120)).is_none());
    }

    #[test]
    fn honors_explicit_cooldown_override() {
        let previous = std::env::var_os(COOLDOWN_ENV);
        unsafe { std::env::set_var(COOLDOWN_ENV, "90") };
        assert_eq!(
            cooldown_duration("limit", UNIX_EPOCH),
            Duration::from_secs(90)
        );
        unsafe { std::env::set_var(COOLDOWN_ENV, "not-a-number") };
        assert_eq!(cooldown_duration("limit", UNIX_EPOCH), DEFAULT_COOLDOWN);
        unsafe { std::env::set_var(COOLDOWN_ENV, "999999999") };
        assert_eq!(cooldown_duration("limit", UNIX_EPOCH), MAX_COOLDOWN);
        match previous {
            Some(value) => unsafe { std::env::set_var(COOLDOWN_ENV, value) },
            None => unsafe { std::env::remove_var(COOLDOWN_ENV) },
        }
    }

    #[test]
    fn records_and_loads_codex_cooldown_from_home_cache() {
        let root = tempfile::tempdir().expect("cooldown home");
        let previous_home = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", root.path()) };
        let now = UNIX_EPOCH + Duration::from_secs(2_000);
        assert!(current_cache_path().is_some());
        assert!(record_codex_app_server_limit_at(None, "limit", now).is_none());
        let path = current_cache_path().expect("home cache");
        assert!(record_codex_app_server_limit_at(Some(&path), "limit", now).is_some());
        assert!(codex_app_server_is_cooling_down_at(Some(&path), now));
        assert!(!codex_app_server_is_cooling_down_at(None, now));
        let wrong_version = UsageLimitCooldown {
            version: CACHE_VERSION + 1,
            backend: "codex-app-server".to_owned(),
            until_unix_seconds: 9_999,
            message: "limit".to_owned(),
            recorded_unix_seconds: 2_000,
        };
        write_cooldown(&path, &wrong_version);
        assert!(load_active(&path, now).is_none());
        std::fs::write(&path, b"not-json").expect("corrupt cooldown");
        assert!(load_active(&path, now).is_none());
        assert!(load_active(&root.path().join("missing.json"), now).is_none());
        match previous_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }
}
