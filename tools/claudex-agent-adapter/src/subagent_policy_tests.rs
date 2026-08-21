#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::path::Path;

    use super::*;

    fn clear_last_good() {
        clear_denylist_cache();
    }

    #[test]
    fn merges_sorts_and_deduplicates_configured_and_environment_models() {
        let configured = BTreeSet::from(["gpt-5.6-sol".to_owned()]);
        assert_eq!(
            merged_header(
                &configured,
                Some(OsStr::new(" grok-4.6,gpt-5.6-sol,grok-4.6 "))
            )
            .expect("valid model policy"),
            Some("gpt-5.6-sol,grok-4.6".to_owned())
        );
        assert_eq!(
            merged_header(&configured, None).unwrap(),
            Some("gpt-5.6-sol".to_owned())
        );
        assert_eq!(merged_header(&BTreeSet::new(), None).unwrap(), None);
        assert_eq!(
            merged_header(&BTreeSet::new(), Some(OsStr::new(" , "))).unwrap(),
            None
        );
    }

    #[test]
    fn loads_dedicated_policy_and_resolves_terminal_specific_paths() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let default = root.path().join(CONFIG_RELATIVE_PATH);
        std::fs::create_dir_all(default.parent().unwrap()).unwrap();
        std::fs::write(
            &default,
            r#"{"version":1,"disabledModels":["grok-4.6","gpt-5.6-sol"]}"#,
        )
        .unwrap();
        assert_eq!(
            config_path(None, Some(root.path().as_os_str())).unwrap(),
            Some(default.clone())
        );
        assert_eq!(
            load_config(Some(&default)).into_iter().collect::<Vec<_>>(),
            ["gpt-5.6-sol", "grok-4.6"]
        );

        let shared_local = root
            .path()
            .join(CONFIG_DIR_RELATIVE)
            .join(LOCAL_CONFIG_NAME);
        std::fs::write(
            &shared_local,
            r#"{"version":1,"disabledModels":["shared-local-model"]}"#,
        )
        .unwrap();
        assert_eq!(
            config_path(None, Some(root.path().as_os_str())).unwrap(),
            Some(shared_local)
        );

        if let Some(hostname) = short_hostname() {
            let hostname_local = root
                .path()
                .join(CONFIG_DIR_RELATIVE)
                .join(format!("disabled-subagent-models.{hostname}.local.json"));
            std::fs::write(
                &hostname_local,
                r#"{"version":1,"disabledModels":["hostname-local-model"]}"#,
            )
            .unwrap();
            assert_eq!(
                config_path(None, Some(root.path().as_os_str())).unwrap(),
                Some(hostname_local)
            );
        }

        let alternate = root.path().join("terminal.json");
        std::fs::write(
            &alternate,
            r#"{"version":1,"disabledModels":["qwen3.8-max-preview"]}"#,
        )
        .unwrap();
        assert_eq!(
            config_path(Some(alternate.as_os_str()), None).unwrap(),
            Some(alternate)
        );
        assert_eq!(config_path(None, None).unwrap(), None);
        assert!(config_path(Some(OsStr::new("")), None).is_err());
        assert!(config_path(Some(root.path().as_os_str()), None).is_err());
        assert!(load_config(None).is_empty());
        clear_last_good();
        let missing = root.path().join("missing-disabled-subagent-models.json");
        assert_eq!(
            config_path(Some(missing.as_os_str()), None).unwrap(),
            Some(missing.clone())
        );
        assert_eq!(
            load_config(Some(&missing)).into_iter().collect::<Vec<_>>(),
            [DENY_ALL_SENTINEL]
        );
        let warning = denylist_load_warning().expect("dedicated missing file is fail-closed");
        assert!(warning.contains("denylist file missing"), "{warning}");
        assert!(
            !missing.exists(),
            "non-canonical missing denylist must not be created"
        );
    }

    #[test]
    fn missing_canonical_denylist_is_empty_and_creates_default_file() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(CONFIG_RELATIVE_PATH);
        assert!(load_config(Some(&path)).is_empty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\n  \"version\": 1,\n  \"disabledModels\": []\n}\n"
        );
    }

    #[test]
    fn blank_canonical_denylist_is_empty_and_creates_default_file() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("disabled-subagent-models.json");
        std::fs::write(&path, "  \n").unwrap();
        assert!(load_config(Some(&path)).is_empty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\n  \"version\": 1,\n  \"disabledModels\": []\n}\n"
        );
    }

    #[test]
    fn present_denylist_file_is_loaded() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("disabled-subagent-models.json");
        std::fs::write(
            &path,
            r#"{"version":1,"disabledModels":["grok-4.6","gpt-5.6-sol"]}"#,
        )
        .unwrap();
        assert_eq!(
            load_config(Some(&path)).into_iter().collect::<Vec<_>>(),
            ["gpt-5.6-sol", "grok-4.6"]
        );
    }

    #[test]
    fn malformed_tracked_denylist_is_empty_not_http_fail() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("disabled-subagent-models.json");
        std::fs::write(&path, "not-json").unwrap();
        assert!(load_config(Some(&path)).is_empty());
        let warning = denylist_load_warning().expect("tracked parse failure is visible");
        assert!(warning.contains(&path.display().to_string()), "{warning}");
        assert!(!warning.contains(DENY_ALL_SENTINEL), "{warning}");
    }

    #[test]
    fn malformed_hostname_local_denylist_is_fail_closed() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join("disabled-subagent-models.kkk4oru.local.json");
        std::fs::write(&path, "not-json").unwrap();
        assert_eq!(
            load_config(Some(&path)).into_iter().collect::<Vec<_>>(),
            [DENY_ALL_SENTINEL]
        );
        let warning = denylist_load_warning().expect("dedicated parse failure is fail-closed");
        assert!(
            warning.contains("denylist unavailable at cold start"),
            "{warning}"
        );
        assert!(warning.contains("refusing allow-all"), "{warning}");
        assert!(warning.contains(&path.display().to_string()), "{warning}");
    }

    #[cfg(unix)]
    #[test]
    fn config_path_falls_back_when_hostname_command_fails() {
        use std::os::unix::fs::PermissionsExt;

        const CHILD: &str = "CLAUDEX_SUBAGENT_POLICY_HOSTNAME_FAILURE_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let root = tempfile::tempdir().expect("isolated policy home");
            assert_eq!(
                config_path(None, Some(root.path().as_os_str())).unwrap(),
                Some(root.path().join(CONFIG_RELATIVE_PATH))
            );
            return;
        }

        let fixture = tempfile::tempdir().expect("hostname failure fixture");
        let hostname = fixture.path().join("hostname");
        std::fs::write(&hostname, "#!/bin/sh\nexit 1\n").expect("fake hostname");
        std::fs::set_permissions(&hostname, std::fs::Permissions::from_mode(0o755))
            .expect("executable fake hostname");
        let status = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "subagent_policy::tests::config_path_falls_back_when_hostname_command_fails",
            ])
            .env(CHILD, "1")
            .env("PATH", fixture.path())
            .status()
            .expect("run hostname failure child");
        assert!(status.success(), "hostname failure child failed: {status}");
    }

    #[test]
    fn loads_hostname_local_denylist_with_provider_model_ids() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join("disabled-subagent-models.kkk4oru.local.json");
        std::fs::write(
            &path,
            r#"{
  "version": 1,
  "disabledModels": [
    "opencode-go/deepseek-v4-flash",
    "opencode-go/deepseek-v4-pro",
    "grok-4.6",
    "fugu"
  ]
}
"#,
        )
        .unwrap();
        assert_eq!(
            load_config(Some(&path)).into_iter().collect::<Vec<_>>(),
            [
                "fugu",
                "grok-4.6",
                "opencode-go/deepseek-v4-flash",
                "opencode-go/deepseek-v4-pro"
            ]
        );
    }

    #[test]
    fn retries_a_torn_denylist_read_then_keeps_last_good_policy() {
        clear_last_good();
        let path = Path::new("/tmp/disabled-subagent-models.torn.json");
        let reads = Cell::new(0);
        let responses = [
            "{".to_owned(),
            r#"{"version":1,"disabledModels":["grok-4.6","fugu"]}"#.to_owned(),
        ];
        let models = load_config_from_reader(
            || {
                let attempt = reads.get();
                reads.set(attempt + 1);
                Ok(responses[attempt].clone())
            },
            path,
        );
        assert_eq!(reads.get(), 2);
        assert_eq!(models.into_iter().collect::<Vec<_>>(), ["fugu", "grok-4.6"]);

        let cached = load_config_from_reader(|| Ok("not-json".to_owned()), path);
        assert_eq!(cached.into_iter().collect::<Vec<_>>(), ["fugu", "grok-4.6"]);
        let warning = denylist_load_warning().expect("stale denylist must be user-visible");
        assert!(
            warning.contains("last-known-good"),
            "stale warning should name last-known-good: {warning}"
        );
    }

    #[test]
    fn uninitialized_denylist_read_failure_is_fail_closed() {
        clear_last_good();
        let path = Path::new("/tmp/disabled-subagent-models.uninitialized.json");
        let models = load_config_from_reader(|| Ok("not-json".to_owned()), path);
        assert_eq!(models.into_iter().collect::<Vec<_>>(), [DENY_ALL_SENTINEL]);
        let warning = denylist_load_warning().expect("uninitialized dedicated is fail-closed");
        assert!(
            warning.contains("denylist unavailable at cold start"),
            "{warning}"
        );
        assert!(warning.contains("refusing allow-all"), "{warning}");
    }

    #[test]
    fn unset_denylist_survives_later_read_failure() {
        clear_last_good();
        let path = Path::new("/tmp/disabled-subagent-models.unset.json");
        let unset = load_config_from_reader(
            || Ok(r#"{"version":1,"disabledModels":[]}"#.to_owned()),
            path,
        );
        assert!(unset.is_empty());
        let cached = load_config_from_reader(|| Ok("not-json".to_owned()), path);
        assert!(cached.is_empty());
        let warning = denylist_load_warning().expect("stale empty last-good must still surface");
        assert!(warning.contains("last-known-good"), "{warning}");
    }

    #[test]
    fn rejects_invalid_dedicated_policy_files_until_retries_exhaust() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let version_path = root.path().join("policy-version.json");
        std::fs::write(&version_path, r#"{"version":2,"disabledModels":[]}"#).unwrap();
        assert_eq!(
            load_config(Some(&version_path))
                .into_iter()
                .collect::<Vec<_>>(),
            [DENY_ALL_SENTINEL]
        );
        clear_last_good();
        let duplicate_path = root.path().join("policy-duplicate.json");
        std::fs::write(
            &duplicate_path,
            r#"{"version":1,"disabledModels":["same","same"]}"#,
        )
        .unwrap();
        assert_eq!(
            load_config(Some(&duplicate_path))
                .into_iter()
                .collect::<Vec<_>>(),
            [DENY_ALL_SENTINEL]
        );
        let warning = denylist_load_warning().expect("invalid dedicated policy is fail-closed");
        assert!(
            warning.contains("denylist unavailable at cold start"),
            "{warning}"
        );
    }

    #[test]
    #[expect(clippy::excessive_nesting, reason = "retry callback fixture intentionally captures sequential attempts")]
    fn retries_transient_io_then_loads_dedicated_policy() {
        clear_last_good();
        let path = Path::new("/tmp/disabled-subagent-models.emfile.json");
        let reads = Cell::new(0);
        let models = load_config_from_reader(
            || {
                let attempt = reads.get();
                reads.set(attempt + 1);
                if attempt == 0 {
                    return Err(std::io::Error::from(std::io::ErrorKind::Interrupted).into());
                }
                Ok(r#"{"version":1,"disabledModels":["grok-4.6"]}"#.to_owned())
            },
            path,
        );
        assert_eq!(reads.get(), 2);
        assert_eq!(models.into_iter().collect::<Vec<_>>(), ["grok-4.6"]);
    }

    #[test]
    fn last_good_survives_process_restart_from_disk() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join("disabled-subagent-models.kkk4oru.local.json");
        std::fs::write(
            &path,
            r#"{"version":1,"disabledModels":["hostname-local-model"]}"#,
        )
        .unwrap();
        assert_eq!(
            load_config(Some(&path)).into_iter().collect::<Vec<_>>(),
            ["hostname-local-model"]
        );
        clear_memory_keep_disk_last_good();
        std::fs::write(&path, "{").unwrap();
        assert_eq!(
            load_config(Some(&path)).into_iter().collect::<Vec<_>>(),
            ["hostname-local-model"]
        );
        let warning = denylist_load_warning().expect("stale disk last-good must be visible");
        assert!(warning.contains("last-known-good"), "{warning}");
    }

    #[test]
    fn hostname_local_still_disables_when_tracked_file_is_missing() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let config_dir = root.path().join(CONFIG_DIR_RELATIVE);
        std::fs::create_dir_all(&config_dir).unwrap();
        let hostname = short_hostname().expect("hostname");
        let hostname_local =
            config_dir.join(format!("disabled-subagent-models.{hostname}.local.json"));
        std::fs::write(
            &hostname_local,
            r#"{"version":1,"disabledModels":["hostname-local-model"]}"#,
        )
        .unwrap();
        let selected = config_path(None, Some(root.path().as_os_str()))
            .unwrap()
            .expect("hostname-local is preferred over missing tracked file");
        assert_eq!(selected, hostname_local);
        assert!(!root.path().join(CONFIG_RELATIVE_PATH).exists());
        assert_eq!(
            load_config(Some(&selected)).into_iter().collect::<Vec<_>>(),
            ["hostname-local-model"]
        );
    }

    #[test]
    fn reads_request_header_and_rejects_invalid_model_ids() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HEADER_NAME,
            "qwen3.8-max-preview,gpt-5.6-sol".parse().unwrap(),
        );
        assert_eq!(
            request_models(&headers)
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["gpt-5.6-sol", "qwen3.8-max-preview"]
        );
        assert!(parse("model with spaces").is_err());
        assert!(!valid_model_id(""));
        assert!(!valid_model_id("モデル"));
        assert!(!valid_model_id("model\n"));
    }

    #[test]
    fn fail_closed_sentinel_disables_every_subagent_model() {
        let disabled = BTreeSet::from([DENY_ALL_SENTINEL.to_owned()]);
        assert!(model_is_disabled(&disabled, "grok-4.6"));
        assert!(model_is_disabled(&disabled, "gpt-5.6-sol"));
        assert!(!model_is_disabled(&BTreeSet::new(), "grok-4.6"));
        assert!(model_is_disabled(
            &BTreeSet::from(["grok-4.6".to_owned()]),
            "grok-4.6"
        ));
    }

    #[test]
    fn active_models_preserves_the_machine_policy_shape() {
        let configured = BTreeSet::from(["grok-4.6".to_owned()]);
        let request = BTreeSet::from(["qwen3.8-max-preview".to_owned()]);
        let mut merged = configured;
        merged.extend(request);
        assert_eq!(
            merged.into_iter().collect::<Vec<_>>(),
            ["grok-4.6", "qwen3.8-max-preview"]
        );
    }
}
