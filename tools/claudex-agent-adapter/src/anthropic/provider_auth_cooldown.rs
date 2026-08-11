use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};

pub(super) const CACHE_FILE_NAME: &str = "provider-auth-cooldown.json";
pub(super) const CACHE_VERSION: u8 = 1;
pub(super) const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30 * 60);
/// Provider 429 / weekly-cap cool-downs outlive short auth blips. Ollama Cloud
/// often stays dark for hours after the first retry storm; 30 minutes caused
/// automatic re-selection loops.
pub(super) const DEFAULT_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(4 * 60 * 60);
pub(super) const MAX_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);
pub(super) const COOLDOWN_ENV: &str = "CLAUDEX_PROVIDER_AUTH_COOLDOWN_SECONDS";
pub(super) const RATE_LIMIT_COOLDOWN_ENV: &str = "CLAUDEX_PROVIDER_RATE_LIMIT_COOLDOWN_SECONDS";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AuthCooldownEntry {
    pub(super) until_unix_seconds: u64,
    pub(super) message: String,
    pub(super) recorded_unix_seconds: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AuthCooldownCache {
    pub(super) version: u8,
    #[serde(default)]
    pub(super) entries: BTreeMap<String, AuthCooldownEntry>,
}

pub(crate) fn cache_path_for_home(home: impl AsRef<Path>) -> PathBuf {
    home.as_ref().join(".cache/claudex").join(CACHE_FILE_NAME)
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn current_cache_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(cache_path_for_home)
}

pub(crate) fn record_at(
    path: Option<&Path>,
    scope: &str,
    message: &str,
    now: SystemTime,
) -> Option<PathBuf> {
    record_with_duration(path, scope, message, now, cooldown_duration())
}

pub(crate) fn record_rate_limit_at(
    path: Option<&Path>,
    scope: &str,
    message: &str,
    now: SystemTime,
) -> Option<PathBuf> {
    record_with_duration(path, scope, message, now, rate_limit_cooldown_duration())
}

fn record_with_duration(
    path: Option<&Path>,
    scope: &str,
    message: &str,
    now: SystemTime,
    duration: Duration,
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
    let until = unix_seconds(now + duration);
    // Keep the longer of an existing cool-down and this one so a later auth
    // blip cannot shorten an active rate-limit window.
    let until_unix_seconds = cache
        .entries
        .get(scope)
        .map(|entry| entry.until_unix_seconds.max(until))
        .unwrap_or(until);
    cache.entries.insert(
        scope.to_owned(),
        AuthCooldownEntry {
            until_unix_seconds,
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

#[path = "provider_auth_cooldown_io.rs"]
mod io;
use io::{
    cooldown_duration, load_cache, prune_expired, rate_limit_cooldown_duration, unix_seconds,
    write_cache,
};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "provider_auth_cooldown_tests.rs"]
mod tests;
