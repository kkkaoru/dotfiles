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
        assert!(
            load_active(&path, UNIX_EPOCH + Duration::from_secs(1_000 + 59)).is_some(),
            "one second before expiry must still block"
        );
        assert!(
            load_active(&path, UNIX_EPOCH + Duration::from_secs(1_000 + 60)).is_none(),
            "exactly at untilUnixSeconds must already be expired"
        );
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
