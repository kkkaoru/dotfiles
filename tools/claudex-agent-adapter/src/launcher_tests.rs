#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::agent_backend::BackendKind;

    fn config() -> ServiceConfig {
        ServiceConfig {
            options: AdapterOptions {
                routes: vec![BackendRoute::new(
                    "test-model",
                    BackendKind::CodexAppServer,
                )],
                listen: "127.0.0.1:8318".parse().expect("default listen"),
                model: "test-model".to_owned(),
                subscription_max_processes: 20,
                subscription_timeout_minutes: 120,
            },
            token: LOCAL_TOKEN.to_owned(),
            executable: PathBuf::from("/tmp/adapter"),
            log_path: PathBuf::from("/tmp/adapter.log"),
        }
    }

    #[test]
    fn formats_the_listener_and_matches_all_health_settings() {
        let base_config = config();
        assert_eq!(base_config.base_url(), "http://127.0.0.1:8318");
        assert!(base_config.matches(&healthy(&base_config)));
        let mut alternate_main = config();
        alternate_main.options.model = "alternate-model".to_owned();
        assert!(alternate_main.matches(&healthy(&base_config)));
    }

    #[test]
    fn connects_to_loopback_for_unspecified_bind_addresses() {
        let mut config = config();
        config.options.listen = "0.0.0.0:9000".parse().expect("IPv4 listener");
        assert_eq!(config.base_url(), "http://127.0.0.1:9000");
        config.options.listen = "[::]:9000".parse().expect("IPv6 listener");
        assert_eq!(config.base_url(), "http://[::1]:9000");
    }

    #[test]
    fn rejects_a_second_main_model_argument() {
        assert!(reject_model_override(&["--model".into(), "other".into()]).is_err());
        assert!(reject_model_override(&["--model=other".into()]).is_err());
        assert!(reject_model_override(&["--continue".into()]).is_ok());
    }

    fn healthy(config: &ServiceConfig) -> Health {
        Health {
            status: "ok".to_owned(),
            pid: Some(42),
            protocol_version: ADAPTER_PROTOCOL_VERSION,
            _build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            backend_routes: route_descriptions(&config.options.routes),
            subscription_max_processes: 20,
            subscription_timeout_minutes: 120,
        }
    }

    #[test]
    fn rejects_each_stale_health_dimension() {
        let config = config();
        let mut stale = Vec::new();
        let mut health = healthy(&config);
        health.status = "unavailable".to_owned();
        stale.push(health);
        let mut health = healthy(&config);
        health.protocol_version += 1;
        stale.push(health);
        let mut health = healthy(&config);
        health.subscription_max_processes = 7;
        stale.push(health);
        let mut health = healthy(&config);
        health.subscription_timeout_minutes = 45;
        stale.push(health);
        for health in stale {
            assert!(!config.matches(&health));
        }

        let mut compatible_build = healthy(&config);
        compatible_build._build_id = "newer-compatible-build".to_owned();
        assert!(config.matches(&compatible_build));
    }

    #[test]
    fn relays_non_warning_stderr_bytes() {
        let mut output = Vec::new();
        let advisor_warning = "Advisor disabled — base model 'test-model' has no advisor rank\n";
        let connector_warning =
            "claude.ai connectors are disabled because another auth source takes precedence\n";
        let input = format!("{advisor_warning}{connector_warning}kept warning\n");
        relay_filtered(input.as_bytes(), "test-model", &mut output).expect("relay fixture");
        assert_eq!(output, b"kept warning\n");
    }

    #[cfg(unix)]
    #[test]
    fn converts_signal_exit_statuses() {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(exit_code(std::process::ExitStatus::from_raw(9)), 137);
        assert_eq!(exit_code(std::process::ExitStatus::from_raw(0)), 0);
    }

    #[tokio::test]
    async fn handles_absent_legacy_processes_and_readiness_timeout() {
        let mut config = config();
        config.options.listen = "127.0.0.1:1".parse().expect("closed test listener");
        config.executable = PathBuf::from("/definitely/missing/adapter");
        stop_stale(&config, None).await;
        terminate(u32::MAX);
        let error = wait_until_ready_with(
            &reqwest::Client::new(),
            &config,
            Duration::from_millis(1),
            Duration::from_millis(1),
            Duration::from_millis(1),
        )
        .await
        .expect_err("unreachable adapter must time out");
        assert!(error.to_string().contains("failed to start"));
        stop_stale(&config, Some(std::process::id())).await;
    }

    #[test]
    fn reports_adapter_log_configuration_errors() {
        let mut config = config();
        config.log_path = PathBuf::new();
        let error = start_adapter(&config).expect_err("parentless log path must fail");
        assert!(error.to_string().contains("adapter log has no parent"));

        let root = tempfile::tempdir().expect("log fixture");
        let occupied = root.path().join("occupied");
        std::fs::write(&occupied, "file").expect("occupied path");
        config.log_path = occupied.join("adapter.log");
        assert!(start_adapter(&config).is_err());

        let directory_log = root.path().join("directory-log");
        std::fs::create_dir(&directory_log).expect("directory log");
        config.log_path = directory_log;
        assert!(start_adapter(&config).is_err());
    }
}
