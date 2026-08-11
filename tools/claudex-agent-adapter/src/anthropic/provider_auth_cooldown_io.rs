use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::{
    AuthCooldownCache, COOLDOWN_ENV, DEFAULT_COOLDOWN, DEFAULT_RATE_LIMIT_COOLDOWN, MAX_COOLDOWN,
    RATE_LIMIT_COOLDOWN_ENV,
};

pub(super) fn load_cache(path: &Path) -> Option<AuthCooldownCache> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub(super) fn prune_expired(cache: &mut AuthCooldownCache, now: SystemTime) {
    let now = unix_seconds(now);
    cache
        .entries
        .retain(|_, entry| now < entry.until_unix_seconds);
}

pub(super) fn cooldown_duration() -> Duration {
    env_cooldown(COOLDOWN_ENV).unwrap_or(DEFAULT_COOLDOWN.min(MAX_COOLDOWN))
}

pub(super) fn rate_limit_cooldown_duration() -> Duration {
    env_cooldown(RATE_LIMIT_COOLDOWN_ENV).unwrap_or(DEFAULT_RATE_LIMIT_COOLDOWN.min(MAX_COOLDOWN))
}

pub(super) fn env_cooldown(name: &str) -> Option<Duration> {
    std::env::var(name)
        .ok()
        .and_then(|seconds| seconds.parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.min(MAX_COOLDOWN.as_secs())))
}

pub(super) fn write_cache(path: &Path, cache: &AuthCooldownCache) {
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

pub(super) fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
