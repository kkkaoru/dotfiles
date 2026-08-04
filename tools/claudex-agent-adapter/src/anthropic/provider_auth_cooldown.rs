use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

const CACHE_FILE_NAME: &str = "provider-auth-cooldown.json";
const CACHE_VERSION: u8 = 1;
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30 * 60);
const MAX_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);
const COOLDOWN_ENV: &str = "CLAUDEX_PROVIDER_AUTH_COOLDOWN_SECONDS";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthCooldownEntry {
    until_unix_seconds: u64,
    message: String,
    recorded_unix_seconds: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthCooldownCache {
    version: u8,
    #[serde(default)]
    entries: BTreeMap<String, AuthCooldownEntry>,
}

pub(crate) fn cache_path_for_home(home: impl AsRef<Path>) -> PathBuf {
    home.as_ref().join(".cache/claudex").join(CACHE_FILE_NAME)
}

pub(crate) fn current_cache_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(cache_path_for_home)
}

pub(crate) fn record_at(
    path: Option<&Path>,
    scope: &str,
    message: &str,
    now: SystemTime,
) -> Option<PathBuf> {
    if scope.is_empty() {
        return None;
    }
    let path = path?;
    let mut cache = load_cache(path).unwrap_or_else(|| AuthCooldownCache {
        version: CACHE_VERSION,
        entries: BTreeMap::new(),
    });
    cache.version = CACHE_VERSION;
    cache.entries.insert(
        scope.to_owned(),
        AuthCooldownEntry {
            until_unix_seconds: unix_seconds(now + cooldown_duration()),
            message: message.to_owned(),
            recorded_unix_seconds: unix_seconds(now),
        },
    );
    prune_expired(&mut cache, now);
    write_cache(path, &cache);
    Some(path.to_path_buf())
}

pub(crate) fn scope_is_cooling_down_at(path: Option<&Path>, scope: &str, now: SystemTime) -> bool {
    if scope.is_empty() {
        return false;
    }
    path.and_then(load_cache).is_some_and(|cache| {
        cache.version == CACHE_VERSION
            && cache
                .entries
                .get(scope)
                .is_some_and(|entry| unix_seconds(now) < entry.until_unix_seconds)
    })
}

fn load_cache(path: &Path) -> Option<AuthCooldownCache> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn prune_expired(cache: &mut AuthCooldownCache, now: SystemTime) {
    let now = unix_seconds(now);
    cache
        .entries
        .retain(|_, entry| now < entry.until_unix_seconds);
}

fn cooldown_duration() -> Duration {
    if let Ok(seconds) = std::env::var(COOLDOWN_ENV)
        && let Ok(seconds) = seconds.parse::<u64>()
    {
        return Duration::from_secs(seconds.min(MAX_COOLDOWN.as_secs()));
    }
    DEFAULT_COOLDOWN.min(MAX_COOLDOWN)
}

fn write_cache(path: &Path, cache: &AuthCooldownCache) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(payload) = serde_json::to_vec_pretty(cache) else {
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
    fn records_and_expires_provider_scoped_auth_cooldown() {
        let root = tempfile::tempdir().expect("auth cooldown fixture");
        let path = cache_path_for_home(root.path());
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        assert!(record_at(Some(&path), "sakana", "Invalid API key", now).is_some());
        assert!(scope_is_cooling_down_at(Some(&path), "sakana", now));
        assert!(!scope_is_cooling_down_at(Some(&path), "other", now));
        assert!(!scope_is_cooling_down_at(
            Some(&path),
            "sakana",
            now + DEFAULT_COOLDOWN + Duration::from_secs(1)
        ));
    }

    #[test]
    fn honors_explicit_auth_cooldown_override() {
        let previous = std::env::var_os(COOLDOWN_ENV);
        unsafe { std::env::set_var(COOLDOWN_ENV, "45") };
        assert_eq!(cooldown_duration(), Duration::from_secs(45));
        match previous {
            Some(value) => unsafe { std::env::set_var(COOLDOWN_ENV, value) },
            None => unsafe { std::env::remove_var(COOLDOWN_ENV) },
        }
    }
}
