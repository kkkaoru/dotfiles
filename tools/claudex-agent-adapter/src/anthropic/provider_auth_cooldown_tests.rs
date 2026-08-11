use super::*;
use std::time::UNIX_EPOCH;

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
fn rate_limit_cooldown_outlives_default_auth_window() {
    let root = tempfile::tempdir().expect("rate limit cooldown fixture");
    let path = cache_path_for_home(root.path());
    let now = UNIX_EPOCH + Duration::from_secs(1_000);
    assert!(
        record_rate_limit_at(Some(&path), "ollama", "429 Too Many Requests", now).is_some()
    );
    assert!(scope_is_cooling_down_at(
        Some(&path),
        "ollama",
        now + DEFAULT_COOLDOWN + Duration::from_secs(1)
    ));
    assert!(!scope_is_cooling_down_at(
        Some(&path),
        "ollama",
        now + DEFAULT_RATE_LIMIT_COOLDOWN + Duration::from_secs(1)
    ));
}

#[test]
fn record_without_path_or_scope_is_a_noop() {
    let now = UNIX_EPOCH + Duration::from_secs(1_000);
    assert!(record_at(None, "sakana", "Invalid API key", now).is_none());
    assert!(record_rate_limit_at(None, "ollama", "429", now).is_none());
    assert!(record_at(Some(Path::new("/tmp/unused")), "", "Invalid API key", now).is_none());
    assert!(!scope_is_cooling_down_at(None, "sakana", now));
    assert!(!scope_is_cooling_down_at(
        Some(Path::new("/tmp/unused")),
        "",
        now
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

#[test]
fn ignores_auth_cooldown_cache_with_unexpected_version() {
    let root = tempfile::tempdir().expect("auth cooldown version fixture");
    let path = cache_path_for_home(root.path());
    std::fs::create_dir_all(path.parent().expect("cache parent")).expect("cache dir");
    std::fs::write(
        &path,
        r#"{"version":99,"entries":{"sakana":{"untilUnixSeconds":9999999999,"message":"Invalid API key","recordedUnixSeconds":1}}}"#,
    )
    .expect("write stale version cache");
    let now = UNIX_EPOCH + Duration::from_secs(1_000);
    assert!(!scope_is_cooling_down_at(Some(&path), "sakana", now));
}

#[test]
fn write_cache_skips_rename_when_the_target_cannot_be_written() {
    let root = tempfile::tempdir().expect("unwritable cooldown fixture");
    let as_directory = root.path().join("cooldown.json");
    std::fs::create_dir_all(&as_directory).expect("dir target");
    let cache = AuthCooldownCache {
        version: CACHE_VERSION,
        entries: BTreeMap::new(),
    };
    write_cache(&as_directory, &cache);
    write_cache(Path::new("/"), &cache);
}
