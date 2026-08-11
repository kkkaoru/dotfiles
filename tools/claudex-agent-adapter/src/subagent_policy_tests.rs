#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::path::Path;

    use super::*;

    fn clear_last_good() {
        with_last_good(|slot| *slot = None);
    }

    #[test]
    fn merges_sorts_and_deduplicates_configured_and_environment_models() {
        let configured = BTreeSet::from(["gpt-5.6-sol".to_owned()]);
        assert_eq!(
            merged_header(
                &configured,
                Some(OsStr::new(" grok-4.5,gpt-5.6-sol,grok-4.5 "))
            )
            .expect("valid model policy"),
            Some("gpt-5.6-sol,grok-4.5".to_owned())
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
            r#"{"version":1,"disabledModels":["grok-4.5","gpt-5.6-sol"]}"#,
        )
        .unwrap();
        assert_eq!(
            config_path(None, Some(root.path().as_os_str())).unwrap(),
            Some(default.clone())
        );
        assert_eq!(
            load_config(Some(&default))
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            ["gpt-5.6-sol", "grok-4.5"]
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
        assert!(config_path(Some(root.path().join("missing").as_os_str()), None).is_err());
        assert!(load_config(None).unwrap().is_empty());
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
    "grok-4.5",
    "fugu"
  ]
}
"#,
        )
        .unwrap();
        assert_eq!(
            load_config(Some(&path))
                .unwrap()
                .into_iter()
                .collect::<Vec<_>>(),
            [
                "fugu",
                "grok-4.5",
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
            r#"{"version":1,"disabledModels":["grok-4.5","fugu"]}"#.to_owned(),
        ];
        let models = load_config_from_reader(
            || {
                let attempt = reads.get();
                reads.set(attempt + 1);
                Ok(responses[attempt].clone())
            },
            path,
        )
        .expect("torn read should retry");
        assert_eq!(reads.get(), 2);
        assert_eq!(
            models.into_iter().collect::<Vec<_>>(),
            ["fugu", "grok-4.5"]
        );

        let cached = load_config_from_reader(|| Ok("not-json".to_owned()), path)
            .expect("last good policy should survive a later parse failure");
        assert_eq!(cached.into_iter().collect::<Vec<_>>(), ["fugu", "grok-4.5"]);
    }

    #[test]
    fn rejects_invalid_dedicated_policy_files() {
        clear_last_good();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("policy.json");
        for contents in [
            r#"{"version":2,"disabledModels":[]}"#,
            r#"{"version":1,"disabledModels":["invalid model"]}"#,
            r#"{"version":1,"disabledModels":["same","same"]}"#,
            r#"{"version":1,"disabledModels":[],"extra":true}"#,
            "not-json",
        ] {
            assert_rejects_invalid_policy(&path, contents);
        }
    }

    fn assert_rejects_invalid_policy(path: &Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
        let error = load_config(Some(path)).expect_err("invalid policy must fail");
        if contents != "not-json" {
            return;
        }
        let message = error.to_string();
        assert!(
            message.contains("parse disabled SubAgent model config"),
            "{message}"
        );
        assert!(
            message.contains("expected")
                || message.contains("EOF")
                || message.contains("key")
                || message.contains("value"),
            "{message}"
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
    fn active_models_preserves_the_machine_policy_shape() {
        let configured = BTreeSet::from(["grok-4.5".to_owned()]);
        let request = BTreeSet::from(["qwen3.8-max-preview".to_owned()]);
        let mut merged = configured;
        merged.extend(request);
        assert_eq!(
            merged.into_iter().collect::<Vec<_>>(),
            ["grok-4.5", "qwen3.8-max-preview"]
        );
    }
}
