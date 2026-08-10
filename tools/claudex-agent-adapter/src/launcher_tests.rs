#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::excessive_nesting)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::os::unix::process::CommandExt as _;
    use std::{
        collections::BTreeMap,
        io::{Read, Write},
        net::{SocketAddr, TcpListener},
        path::Path,
        process::{Command, Stdio},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    use super::*;
    use crate::agent_backend::BackendKind;
    use serde_json::json;

    fn config() -> ServiceConfig {
        ServiceConfig {
            options: AdapterOptions {
                routes: vec![BackendRoute::new("test-model", BackendKind::CodexAppServer)],
                listen: "127.0.0.1:8318".parse().expect("default listen"),
                model: "test-model".to_owned(),
                subscription_max_processes: 20,
                subscription_timeout_minutes: 120,
                subagent_hard_timeout_seconds: None,
                model_catalog: crate::provider_config::ModelCatalog::default(),
            },
            token: LOCAL_TOKEN.to_owned(),
            codex_config_fingerprint: "test-fingerprint".to_owned(),
            service_config_fingerprint: "service-fingerprint".to_owned(),
            executable: PathBuf::from("/tmp/adapter"),
            log_path: PathBuf::from("/tmp/adapter.log"),
            lock_path: PathBuf::from("/tmp/adapter.lock"),
        }
    }

    #[test]
    fn formats_the_listener_and_matches_all_health_settings() {
        let base_config = config();
        assert_eq!(base_config.base_url(), "http://127.0.0.1:8318");
        assert!(base_config.matches(&healthy(&base_config)));
        let mut alternate_main = config();
        alternate_main.options.model = "alternate-model".to_owned();
        assert!(!alternate_main.matches(&healthy(&base_config)));
    }

    #[test]
    fn rejects_health_from_a_daemon_with_stale_codex_configuration() {
        let config = config();
        let mut stale = healthy(&config);
        stale.codex_config_fingerprint = "stale-fingerprint".to_owned();
        assert!(!config.matches(&stale));
    }

    #[test]
    fn connects_to_loopback_for_unspecified_bind_addresses() {
        let mut config = config();
        config.options.listen = "0.0.0.0:9000".parse().expect("IPv4 listener");
        assert_eq!(config.base_url(), "http://127.0.0.1:9000");
        config.options.listen = "[::]:9000".parse().expect("IPv6 listener");
        assert_eq!(config.base_url(), "http://[::1]:9000");
        config.options.listen = "[::1]:9000".parse().expect("IPv6 loopback listener");
        assert_eq!(config.base_url(), "http://[::1]:9000");
    }

    #[test]
    fn requires_authentication_only_for_untrusted_listeners_with_the_local_token() {
        let loopback = "127.0.0.1:8318".parse().expect("loopback listener");
        let public = "0.0.0.0:8318".parse().expect("public listener");
        assert!(!requires_authentication(&loopback, LOCAL_TOKEN));
        assert!(!requires_authentication(&public, "real-token"));
        assert!(requires_authentication(&public, LOCAL_TOKEN));
    }

    #[test]
    fn routes_logs_per_listen_address() {
        let base = std::path::PathBuf::from("/tmp/claudex-log-cache");
        let listen_1 = "127.0.0.1:8318".parse().expect("listen one");
        let listen_2 = "127.0.0.1:18319".parse().expect("listen two");
        assert_ne!(
            super::launcher_logs::adapter_log_path(&base, &listen_1),
            super::launcher_logs::adapter_log_path(&base, &listen_2)
        );
        assert_eq!(
            super::launcher_logs::adapter_log_path(&base, &listen_1)
                .file_name()
                .expect("name one"),
            "adapter.127_0_0_1_8318.log"
        );
    }

    #[test]
    fn archives_existing_logs_and_writes_a_header() {
        let root = tempfile::tempdir().expect("log archive fixture");
        let missing = root.path().join("missing.log");
        super::launcher_logs::archive_previous_log(&missing).expect("missing log is harmless");
        let log = root.path().join("adapter.log");
        std::fs::write(&log, "old").expect("old log");
        super::launcher_logs::archive_previous_log(&log).expect("archive log");
        assert!(!log.exists());
        let archived = std::fs::read_dir(root.path())
            .expect("archive directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path != &log)
            .expect("archived log");
        assert_eq!(std::fs::read_to_string(archived).unwrap(), "old");

        let mut output = Vec::new();
        super::launcher_logs::write_adapter_log_header(
            &mut output,
            "model",
            &"127.0.0.1:8318".parse().unwrap(),
            7,
        )
        .expect("log header");
        assert!(String::from_utf8(output).unwrap().contains("token_len=7"));
    }

    #[test]
    fn serializes_launchers_that_can_compete_for_the_same_port() {
        let base = std::path::PathBuf::from("/tmp/claudex-lock-cache");
        let loopback = "127.0.0.1:8318".parse().expect("loopback listener");
        let wildcard = "0.0.0.0:8318".parse().expect("wildcard listener");
        assert_eq!(
            super::launcher_logs::adapter_lock_path(&base, &loopback),
            super::launcher_logs::adapter_lock_path(&base, &wildcard)
        );
    }

    #[test]
    fn session_lock_paths_are_stable_and_private_to_each_resume_id() {
        let base = std::path::PathBuf::from("/tmp/claudex-session-lock-cache");
        let first = super::launcher_logs::session_lock_path(&base, "session-a");
        assert_eq!(
            first,
            super::launcher_logs::session_lock_path(&base, "session-a")
        );
        assert_ne!(
            first,
            super::launcher_logs::session_lock_path(&base, "session-b")
        );
        assert!(
            first
                .file_name()
                .is_some_and(|name| name != "session-a.lock")
        );
    }

    #[test]
    fn acquires_launcher_lock_and_rejects_a_parentless_path() {
        let root = tempfile::tempdir().expect("lock fixture");
        let lock_path = root.path().join("adapter.lock");
        let guard = super::launcher_lock::acquire(&lock_path).expect("lock acquisition");
        assert!(lock_path.exists());
        drop(guard);
        assert!(super::launcher_lock::acquire(Path::new("")).is_err());
    }

    #[test]
    fn try_acquires_session_lock_without_waiting_and_releases_on_drop() {
        let root = tempfile::tempdir().expect("session lock fixture");
        let path = root.path().join("session.lock");
        let first = super::launcher_lock::try_acquire(&path)
            .expect("first session lock acquisition")
            .expect("first owner");
        assert!(
            super::launcher_lock::try_acquire(&path)
                .expect("second session lock probe")
                .is_none()
        );
        drop(first);
        assert!(
            super::launcher_lock::try_acquire(&path)
                .expect("session lock after owner exit")
                .is_some()
        );
    }

    #[test]
    fn reports_an_exclusive_lock_error_for_an_invalid_file_descriptor() {
        let error = super::launcher_lock::lock_file_descriptor(-1)
            .expect_err("invalid file descriptor must fail");
        assert!(error.to_string().contains("lock launcher state"));
    }

    #[test]
    fn rejects_a_second_main_model_argument() {
        assert!(reject_model_override(&["--model".into(), "other".into()]).is_err());
        assert!(reject_model_override(&["--model=other".into()]).is_err());
        assert!(reject_model_override(&["--continue".into()]).is_ok());
    }

    #[test]
    fn daemon_arguments_preserve_configured_worker_routes() {
        let mut config = config();
        config
            .options
            .model_catalog
            .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
                "claudex-grok".to_owned(),
                "grok-4.5".to_owned(),
                "high".to_owned(),
            )])
            .expect("worker route");
        let arguments = daemon_arguments(&config.options)
            .into_iter()
            .map(|argument| argument.into_string().expect("UTF-8 argument"))
            .collect::<Vec<_>>();
        assert!(
            arguments.windows(2).any(|pair| {
                pair[0] == "--worker-route-json" && pair[1].contains("claudex-grok")
            })
        );

        config
            .options
            .model_catalog
            .set_search_worker_routes(vec![crate::provider_config::WorkerRoute::new(
                "claudex-search".to_owned(),
                "gpt-search".to_owned(),
                "xhigh".to_owned(),
            )])
            .expect("search worker route");
        config
            .options
            .model_catalog
            .set_selectable_models(vec!["gpt-5.6-terra".to_owned()]);
        let arguments = daemon_arguments(&config.options)
            .into_iter()
            .map(|argument| argument.into_string().expect("UTF-8 argument"))
            .collect::<Vec<_>>();
        assert!(arguments.windows(2).any(|pair| {
            pair[0] == "--search-worker-route-json" && pair[1].contains("claudex-search")
        }));
        assert!(
            arguments
                .windows(2)
                .any(|pair| { pair[0] == "--selectable-model" && pair[1] == "gpt-5.6-terra" })
        );

        config.options.model.clear();
        let arguments = daemon_arguments(&config.options)
            .into_iter()
            .map(|argument| argument.into_string().expect("UTF-8 argument"))
            .collect::<Vec<_>>();
        assert_eq!(arguments.first().map(String::as_str), Some("serve"));
        let wait_arguments = hot_swap_wait_arguments(&config.options)
            .into_iter()
            .map(|argument| argument.into_string().expect("UTF-8 argument"))
            .collect::<Vec<_>>();
        assert_eq!(
            wait_arguments
                .get(..2)
                .map(|prefix| prefix.iter().map(String::as_str).collect::<Vec<_>>()),
            Some(vec!["hot-swap", "--wait-idle"])
        );
        assert!(!arguments.iter().any(|argument| argument == "--model"));
        assert!(
            arguments.windows(2).any(|pair| {
                pair[0] == "--worker-route-json" && pair[1].contains("claudex-grok")
            })
        );
        assert!(arguments.windows(2).any(|pair| {
            pair[0] == "--search-worker-route-json" && pair[1].contains("claudex-search")
        }));
        assert!(
            arguments
                .windows(2)
                .any(|pair| { pair[0] == "--selectable-model" && pair[1] == "gpt-5.6-terra" })
        );
    }

    fn healthy(config: &ServiceConfig) -> Health {
        Health {
            status: "ok".to_owned(),
            pid: Some(42),
            protocol_version: ADAPTER_PROTOCOL_VERSION,
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            model: config.options.model.clone(),
            codex_config_fingerprint: config.codex_config_fingerprint.clone(),
            backend_routes: route_descriptions(&config.options.routes),
            worker_routes: worker_route_descriptions(&config.options.model_catalog),
            search_worker_routes: search_worker_route_descriptions(&config.options.model_catalog),
            subscription_max_processes: 20,
            subscription_timeout_minutes: 120,
            subagent_hard_timeout_seconds: None,
            service_config_fingerprint: config.service_config_fingerprint.clone(),
            recovery_generation: None,
            active_http_requests: 0,
            active_provider_turns: 0,
            active_subagent_models: BTreeMap::new(),
            listener_handover: false,
            listen: None,
            active_claude_session_ids: Vec::new(),
            busy_claude_session_ids: Vec::new(),
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
        let mut health = healthy(&config);
        health.worker_routes.push("stale-worker".to_owned());
        stale.push(health);
        let mut health = healthy(&config);
        health
            .search_worker_routes
            .push("stale-search-worker".to_owned());
        stale.push(health);
        let mut health = healthy(&config);
        health.model = "stale-model".to_owned();
        stale.push(health);
        let mut health = healthy(&config);
        health.service_config_fingerprint = "stale-service".to_owned();
        stale.push(health);
        let mut health = healthy(&config);
        health.backend_routes.push("stale-backend".to_owned());
        stale.push(health);
        let mut health = healthy(&config);
        health.subscription_max_processes = 21;
        stale.push(health);
        let mut health = healthy(&config);
        health.subagent_hard_timeout_seconds = Some(1);
        stale.push(health);
        for health in stale {
            assert!(!config.matches(&health));
        }

        let mut compatible_build = healthy(&config);
        compatible_build.build_id = "newer-compatible-build".to_owned();
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
        let client = reqwest::Client::new();
        handover::release_stale_listener(&client, &config, None)
            .await
            .expect("absent process");
        daemon_process::terminate(u32::MAX);
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
        handover::release_stale_listener(&client, &config, Some(std::process::id()))
            .await
            .expect("current process");
    }

    #[tokio::test]
    async fn recovery_readiness_rejects_each_identity_mismatch_before_authentication() {
        let client = reqwest::Client::new();
        let mut base = config();
        let recovery = super::daemon_start::RecoveryProcess {
            pid: 42,
            generation: "generation".to_owned(),
            protocol_version: ADAPTER_PROTOCOL_VERSION,
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            model: base.options.model.clone(),
            codex_config_fingerprint: base.codex_config_fingerprint.clone(),
            service_config_fingerprint: base.service_config_fingerprint.clone(),
        };
        for mismatch in [
            |health: &mut Health| health.status = "starting".to_owned(),
            |health: &mut Health| health.pid = Some(7),
            |health: &mut Health| health.protocol_version += 1,
            |health: &mut Health| health.build_id = "old-build".to_owned(),
            |health: &mut Health| health.model = "other-model".to_owned(),
            |health: &mut Health| health.codex_config_fingerprint = "old-codex".to_owned(),
            |health: &mut Health| health.service_config_fingerprint = "old-service".to_owned(),
            |health: &mut Health| health.recovery_generation = Some("old-generation".to_owned()),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").expect("recovery readiness listener");
            base.options.listen = listener.local_addr().expect("recovery readiness address");
            let mut health = healthy(&base);
            health.recovery_generation = Some(recovery.generation.clone());
            mismatch(&mut health);
            let server = serve_responses(listener, vec![health_response(&health)]);
            let result = tokio::time::timeout(
                Duration::from_millis(100),
                wait_until_recovery_ready(&client, &base, &recovery),
            )
            .await;
            assert!(
                !matches!(result, Ok(Ok(()))),
                "mismatched recovery health must not authenticate"
            );
            server.join().expect("recovery readiness server");
        }
    }

    #[tokio::test]
    async fn inspects_start_reuse_and_replacement_service_states() {
        let client = reqwest::Client::new();
        let mut absent = config();
        absent.options.listen = unused_listen();
        assert_eq!(
            handover::inspect_service_with(&client, &absent).await,
            handover::ServiceState::Start
        );

        let mut occupied_silent = config();
        let silent = TcpListener::bind("127.0.0.1:0").expect("silent listener");
        occupied_silent.options.listen = silent.local_addr().expect("silent address");
        assert_eq!(
            handover::inspect_service_with(&client, &occupied_silent).await,
            handover::ServiceState::Defer {
                pid: None,
                active_http_requests: 0,
                active_provider_turns: 0,
                active_subagents: 0,
            }
        );
        drop(silent);

        let mut reusable = config();
        let listener = TcpListener::bind("127.0.0.1:0").expect("reuse listener");
        reusable.options.listen = listener.local_addr().expect("reuse address");
        let health = healthy(&reusable);
        let server = serve_responses(
            listener,
            vec![health_response(&health), http_response("200 OK", "{}")],
        );
        assert_eq!(
            handover::inspect_service_with(&client, &reusable).await,
            handover::ServiceState::Reuse
        );
        server.join().expect("reuse server");

        let mut stale = healthy(&reusable);
        stale.build_id = "old-build".to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").expect("stale listener");
        reusable.options.listen = listener.local_addr().expect("stale address");
        let server = serve_responses(listener, vec![health_response(&stale)]);
        assert_eq!(
            handover::inspect_service_with(&client, &reusable).await,
            handover::ServiceState::Replace {
                pid: Some(42),
                recovery_generation: None,
            }
        );
        server.join().expect("stale server");

        let mut active = healthy(&reusable);
        active.build_id = "old-build".to_owned();
        active.active_provider_turns = 1;
        let listener = TcpListener::bind("127.0.0.1:0").expect("active listener");
        reusable.options.listen = listener.local_addr().expect("active address");
        // Only the health request is served. A busy stale adapter must be
        // deferred before the launcher probes auth or sends SIGTERM.
        let server = serve_responses(listener, vec![health_response(&active)]);
        assert_eq!(
            handover::inspect_service_with(&client, &reusable).await,
            handover::ServiceState::Defer {
                pid: Some(42),
                active_http_requests: 0,
                active_provider_turns: 1,
                active_subagents: 0,
            }
        );
        server.join().expect("active server");

        let mut attached = healthy(&reusable);
        attached.build_id = "old-build".to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").expect("attached listener");
        reusable.options.listen = listener.local_addr().expect("attached address");
        let server = serve_responses(listener, vec![health_response(&attached)]);
        assert_eq!(
            handover::inspect_service_with(&client, &reusable).await,
            handover::ServiceState::Replace {
                pid: Some(42),
                recovery_generation: None,
            }
        );
        server.join().expect("attached server");

        let mut hot_busy = healthy(&reusable);
        hot_busy.build_id = "old-build".to_owned();
        hot_busy.active_http_requests = 1;
        let listener = TcpListener::bind("127.0.0.1:0").expect("hot-swap busy listener");
        reusable.options.listen = listener.local_addr().expect("hot-swap busy address");
        let server = serve_responses(listener, vec![health_response(&hot_busy)]);
        assert_eq!(
            handover::inspect_service_with(&client, &reusable).await,
            handover::ServiceState::Defer {
                pid: Some(42),
                active_http_requests: 1,
                active_provider_turns: 0,
                active_subagents: 0,
            }
        );
        server.join().expect("hot-swap busy server");

        let mut subagent_busy = healthy(&reusable);
        subagent_busy.build_id = "old-build".to_owned();
        subagent_busy
            .active_subagent_models
            .insert("auto".to_owned(), 2);
        let listener = TcpListener::bind("127.0.0.1:0").expect("subagent busy listener");
        reusable.options.listen = listener.local_addr().expect("subagent busy address");
        let server = serve_responses(listener, vec![health_response(&subagent_busy)]);
        assert_eq!(
            handover::inspect_service_with(&client, &reusable).await,
            handover::ServiceState::Defer {
                pid: Some(42),
                active_http_requests: 0,
                active_provider_turns: 0,
                active_subagents: 2,
            }
        );
        server.join().expect("subagent busy server");

        let listener = TcpListener::bind("127.0.0.1:0").expect("authentication listener");
        reusable.options.listen = listener.local_addr().expect("authentication address");
        let server = serve_responses(
            listener,
            vec![
                health_response(&healthy(&reusable)),
                http_response("401 Unauthorized", "{}"),
            ],
        );
        assert_eq!(
            handover::inspect_service_with(&client, &reusable).await,
            handover::ServiceState::Replace {
                pid: Some(42),
                recovery_generation: None,
            }
        );
        server.join().expect("authentication server");
    }

    #[tokio::test]
    async fn hot_swap_arms_an_idle_waiter_instead_of_timing_out_while_busy() {
        let root = tempfile::tempdir().expect("hot-swap waiter fixture");
        let mut busy = config();
        let listener = TcpListener::bind("127.0.0.1:0").expect("busy listener");
        busy.options.listen = listener.local_addr().expect("busy address");
        busy.log_path = root.path().join("adapter.log");
        busy.lock_path = root.path().join("adapter.lock");

        let fallback_listener = TcpListener::bind("127.0.0.1:0").expect("fallback listener");
        let fallback_listen = fallback_listener.local_addr().expect("fallback address");
        let fallback = busy.with_listen(fallback_listen);
        std::fs::write(
            root.path()
                .join(format!("fallback.{}.json", busy.options.listen.port())),
            serde_json::json!({
                "listen": fallback_listen,
                "build_id": env!("CLAUDEX_BUILD_ID"),
                "service_config_fingerprint": fallback.service_config_fingerprint,
                "pid": 99,
            })
            .to_string(),
        )
        .expect("write fallback state");

        let mut health = healthy(&busy);
        health.build_id = "old-build".to_owned();
        health.active_http_requests = 1;
        let server = serve_responses(listener, vec![health_response(&health)]);
        let fallback_health = healthy(&fallback);
        let fallback_server = serve_responses(
            fallback_listener,
            vec![
                health_response(&fallback_health),
                http_response("200 OK", "{}"),
            ],
        );
        let events = macos_notify::TestEvents::capture();
        let _spawn = pending_hot_swap::TestSpawnPid::arm(4242);
        let url = ensure::run(&busy, ensure::Mode::HotSwap)
            .await
            .expect("busy hot-swap should arm a waiter and return the current-build fallback");
        assert_eq!(url, fallback.base_url());
        assert_eq!(
            events.take(),
            vec![
                macos_notify::Event::WaitingForIdle {
                    listen: busy.options.listen.to_string(),
                    build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
                    waiter_pid: 4242,
                },
                macos_notify::Event::LiveReady {
                    listen: fallback_listen.to_string(),
                    build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
                    waiting: busy.options.listen.to_string(),
                },
            ],
            "busy hot-swap must notify waiting and that the live generation is ready now"
        );
        let pending = pending_hot_swap::read_state_for_tests(&busy)
            .expect("read pending hot-swap")
            .expect("pending hot-swap state");
        assert_eq!(pending.pid, 4242);
        assert_eq!(pending.build_id, env!("CLAUDEX_BUILD_ID"));
        let live = super::live::read(&busy)
            .expect("read live state")
            .expect("live state after busy hot-swap");
        assert_eq!(live.listen, fallback_listen);
        assert_eq!(live.build_id, env!("CLAUDEX_BUILD_ID"));
        server.join().expect("busy hot-swap server");
        fallback_server.join().expect("fallback server");
    }

    #[tokio::test]
    async fn wait_idle_and_ensure_reuse_a_current_healthy_listener() {
        let root = tempfile::tempdir().expect("reuse fixture");
        let mut wait_idle = config();
        let listener = TcpListener::bind("127.0.0.1:0").expect("wait-idle reuse listener");
        wait_idle.options.listen = listener.local_addr().expect("wait-idle address");
        wait_idle.log_path = root.path().join("wait-idle.log");
        wait_idle.lock_path = root.path().join("wait-idle.lock");
        let health = healthy(&wait_idle);
        let events = macos_notify::TestEvents::capture();
        let server = serve_responses(
            listener,
            vec![health_response(&health), http_response("200 OK", "{}")],
        );
        let url = ensure::run(&wait_idle, ensure::Mode::WaitIdle)
            .await
            .expect("wait-idle reuses the current listener");
        assert_eq!(url, wait_idle.base_url());
        assert!(
            events.take().is_empty(),
            "reusing the current build must not notify swap complete"
        );
        server.join().expect("wait-idle reuse server");

        let mut ensure_cfg = config();
        let listener = TcpListener::bind("127.0.0.1:0").expect("ensure reuse listener");
        ensure_cfg.options.listen = listener.local_addr().expect("ensure address");
        ensure_cfg.log_path = root.path().join("ensure.log");
        ensure_cfg.lock_path = root.path().join("ensure.lock");
        let health = healthy(&ensure_cfg);
        let server = serve_responses(
            listener,
            vec![health_response(&health), http_response("200 OK", "{}")],
        );
        let url = ensure::run(&ensure_cfg, ensure::Mode::Ensure)
            .await
            .expect("ensure reuses the current listener");
        assert_eq!(url, ensure_cfg.base_url());
        server.join().expect("ensure reuse server");
    }

    #[tokio::test]
    async fn wait_idle_polls_busy_work_until_the_listener_is_reusable() {
        let root = tempfile::tempdir().expect("wait-idle poll fixture");
        let mut cfg = config();
        let listener = TcpListener::bind("127.0.0.1:0").expect("wait-idle poll listener");
        cfg.options.listen = listener.local_addr().expect("wait-idle poll address");
        cfg.log_path = root.path().join("adapter.log");
        cfg.lock_path = root.path().join("adapter.lock");
        let mut busy = healthy(&cfg);
        busy.build_id = "old-build".to_owned();
        busy.active_http_requests = 1;
        let reusable = healthy(&cfg);
        let server = serve_responses(
            listener,
            vec![
                health_response(&busy),
                health_response(&reusable),
                http_response("200 OK", "{}"),
            ],
        );
        let url = ensure::run(&cfg, ensure::Mode::WaitIdle)
            .await
            .expect("wait-idle reuses after busy work drains");
        assert_eq!(url, cfg.base_url());
        server.join().expect("wait-idle poll server");
    }

    #[tokio::test]
    async fn wait_idle_replace_reuses_once_the_current_build_returns() {
        let root = tempfile::tempdir().expect("wait-idle replace reuse fixture");
        let mut cfg = config();
        let listener = TcpListener::bind("127.0.0.1:0").expect("wait-idle replace listener");
        cfg.options.listen = listener.local_addr().expect("wait-idle replace address");
        cfg.log_path = root.path().join("adapter.log");
        cfg.lock_path = root.path().join("adapter.lock");
        let mut old = healthy(&cfg);
        old.build_id = "old-build".to_owned();
        old.listener_handover = true;
        let mut current = healthy(&cfg);
        current.listener_handover = true;
        let server = serve_responses(
            listener,
            vec![
                health_response(&old),
                health_response(&current),
                http_response("200 OK", "{}"),
            ],
        );
        let url = ensure::run(&cfg, ensure::Mode::WaitIdle)
            .await
            .expect("wait-idle should reuse after the current build comes back");
        assert_eq!(url, cfg.base_url());
        server.join().expect("wait-idle replace reuse server");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn wait_idle_replace_fails_closed_when_live_update_warm_start_never_readies() {
        let root = tempfile::tempdir().expect("wait-idle replace fail fixture");
        let dummy = root.path().join("claudex-agent-adapter");
        std::fs::write(&dummy, "#!/bin/sh\nexit 0\n").expect("dummy adapter");
        std::fs::set_permissions(&dummy, std::fs::Permissions::from_mode(0o755))
            .expect("dummy executable");
        let mut cfg = config();
        cfg.executable = dummy;
        let listener = TcpListener::bind("127.0.0.1:0").expect("stale handover listener");
        cfg.options.listen = listener.local_addr().expect("stale handover address");
        cfg.log_path = root.path().join("adapter.log");
        cfg.lock_path = root.path().join("adapter.lock");
        let mut old = healthy(&cfg);
        old.build_id = "old-build".to_owned();
        old.listener_handover = true;
        let server = serve_responses(
            listener,
            vec![
                health_response(&old),
                health_response(&old),
                health_response(&old),
            ],
        );
        let error = ensure::run(&cfg, ensure::Mode::WaitIdle)
            .await
            .expect_err("warm-start failure must fail the idle waiter");
        assert!(
            error.to_string().contains("wait for warm-start")
                || error.to_string().contains("warm-start")
                || error.to_string().contains("start"),
            "{error:#}"
        );
        server.join().expect("stale handover server");
    }

    #[tokio::test]
    async fn ensure_routes_a_busy_listener_to_an_existing_current_build_fallback() {
        let root = tempfile::tempdir().expect("ensure fallback fixture");
        let mut primary = config();
        let primary_listener = TcpListener::bind("127.0.0.1:0").expect("primary listener");
        primary.options.listen = primary_listener.local_addr().expect("primary address");
        primary.log_path = root.path().join("adapter.log");
        primary.lock_path = root.path().join("adapter.lock");

        let fallback_listener = TcpListener::bind("127.0.0.1:0").expect("fallback listener");
        let fallback_listen = fallback_listener.local_addr().expect("fallback address");
        let fallback = primary.with_listen(fallback_listen);
        std::fs::write(
            root.path()
                .join(format!("fallback.{}.json", primary.options.listen.port())),
            serde_json::json!({
                "listen": fallback_listen,
                "build_id": env!("CLAUDEX_BUILD_ID"),
                "service_config_fingerprint": fallback.service_config_fingerprint,
                "pid": 99,
            })
            .to_string(),
        )
        .expect("write fallback state");

        let mut busy = healthy(&primary);
        busy.build_id = "old-build".to_owned();
        busy.active_http_requests = 1;
        let primary_server = serve_responses(primary_listener, vec![health_response(&busy)]);
        let fallback_health = healthy(&fallback);
        let fallback_server = serve_responses(
            fallback_listener,
            vec![
                health_response(&fallback_health),
                http_response("200 OK", "{}"),
            ],
        );
        let _spawn = pending_hot_swap::TestSpawnPid::arm(4242);
        let url = ensure::run(&primary, ensure::Mode::Ensure)
            .await
            .expect("ensure should reuse the current-build fallback");
        assert_eq!(url, fallback.base_url());
        primary_server.join().expect("primary server");
        fallback_server.join().expect("fallback server");
    }

    #[test]
    fn pending_hot_swap_arm_uses_test_spawn_and_rejects_invalid_state() {
        let root = tempfile::tempdir().expect("pending arm fixture");
        let mut cfg = config();
        cfg.log_path = root.path().join("adapter.log");
        cfg.lock_path = root.path().join("adapter.lock");
        let _spawn = pending_hot_swap::TestSpawnPid::arm(7777);
        let first = pending_hot_swap::arm(&cfg).expect("arm waiter");
        assert_eq!(first.pid(), 7777);
        let again = pending_hot_swap::arm(&cfg).expect("respawn waiter");
        assert_eq!(again.pid(), 7777);

        let path =
            super::launcher_logs::pending_hot_swap_state_path(root.path(), &cfg.options.listen);
        std::fs::write(
            &path,
            br#"{"build_id":"","service_config_fingerprint":"s","pid":1}"#,
        )
        .expect("empty build");
        assert!(pending_hot_swap::read_state_for_tests(&cfg).is_err());
        std::fs::write(
            &path,
            br#"{"build_id":"b","service_config_fingerprint":"s","pid":0}"#,
        )
        .expect("pid zero");
        assert!(pending_hot_swap::read_state_for_tests(&cfg).is_err());
        let recovered = pending_hot_swap::arm(&cfg).expect("corrupt state must not block arm");
        assert_eq!(recovered.pid(), 7777);
    }

    #[test]
    fn spawn_waiter_starts_a_dummy_and_recognizes_its_command_line() {
        let root = tempfile::tempdir().expect("spawn waiter fixture");
        let dummy = root.path().join("claudex-agent-adapter");
        std::fs::write(&dummy, "#!/bin/sh\nexec sleep 30\n").expect("dummy waiter");
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&dummy)
                .expect("dummy metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&dummy, permissions).expect("dummy executable");
        }
        let mut cfg = config();
        cfg.executable = dummy;
        cfg.log_path = root.path().join("adapter.log");
        cfg.lock_path = root.path().join("adapter.lock");
        let first = pending_hot_swap::arm(&cfg).expect("spawn waiter");
        assert!(first.pid() > 1);
        std::thread::sleep(Duration::from_millis(80));
        let again = pending_hot_swap::arm(&cfg).expect("re-arm waiter");
        unsafe {
            libc::kill(-(first.pid() as i32), libc::SIGTERM);
            if again.pid() != first.pid() {
                libc::kill(-(again.pid() as i32), libc::SIGTERM);
            }
        }
        pending_hot_swap::clear_if_current(&cfg);
    }

    #[tokio::test]
    async fn idle_stale_listener_stays_replace_when_recovery_snapshot_is_missing() {
        let root = tempfile::tempdir().expect("wait-idle missing recovery");
        let mut cfg = config();
        let listener = TcpListener::bind("127.0.0.1:0").expect("idle old listener");
        cfg.options.listen = listener.local_addr().expect("idle old address");
        cfg.log_path = root.path().join("adapter.log");
        cfg.lock_path = root.path().join("adapter.lock");
        let mut idle_old = healthy(&cfg);
        idle_old.build_id = "old-build".to_owned();
        idle_old.recovery_generation = Some("v1-missing-generation".to_owned());
        let server = serve_responses(listener, vec![health_response(&idle_old)]);
        let client = reqwest::Client::new();
        let state = handover::inspect_service(&client, &cfg).await;
        match state {
            handover::ServiceState::Replace {
                recovery_generation,
                ..
            } => {
                assert_eq!(
                    recovery_generation.as_deref(),
                    Some("v1-missing-generation")
                );
                assert_eq!(
                    ensure::usable_recovery_generation(&cfg, recovery_generation.as_deref())
                        .expect("missing snapshot is preflight-only"),
                    None,
                    "pruned recovery snapshot must not turn idle Replace into a waiter abort"
                );
            }
            other => panic!("idle stale listener must be Replace, got {other:?}"),
        }
        server.join().expect("idle old listener");
    }

    #[test]
    fn swap_complete_notification_fires_only_after_replace() {
        let root = tempfile::tempdir().expect("swap notify fixture");
        let mut cfg = config();
        cfg.log_path = root.path().join("adapter.log");
        let events = macos_notify::TestEvents::capture();
        assert!(!ensure::listener_was_replaced(
            &handover::ServiceState::Reuse
        ));
        assert!(!ensure::listener_was_replaced(
            &handover::ServiceState::Start
        ));
        assert!(!ensure::listener_was_replaced(
            &handover::ServiceState::Defer {
                pid: None,
                active_http_requests: 1,
                active_provider_turns: 0,
                active_subagents: 0,
            }
        ));
        assert!(ensure::listener_was_replaced(
            &handover::ServiceState::Replace {
                pid: Some(1),
                recovery_generation: None,
            }
        ));
        ensure::notify_swap_if_replaced(false, &cfg);
        assert!(
            events.take().is_empty(),
            "reuse/start must not notify swap complete"
        );
        ensure::notify_swap_if_replaced(true, &cfg);
        assert_eq!(
            events.take(),
            vec![macos_notify::Event::SwapComplete {
                listen: cfg.options.listen.to_string(),
                build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            }]
        );
        ensure::notify_swap_if_replaced(true, &cfg);
        assert!(
            events.take().is_empty(),
            "duplicate swap complete for the same build must not notify again"
        );
    }

    #[test]
    fn idle_replace_failures_keep_the_waiter_alive_in_production() {
        assert!(
            ensure::should_retry_idle_replace(1, None),
            "detached waiters must retry Replace after a failed handover"
        );
        assert!(
            ensure::should_retry_idle_replace(8, None),
            "there is no production retry cap; idle must eventually swap"
        );
        assert!(
            !ensure::should_retry_idle_replace(1, Some(0)),
            "tests fail immediately so dummy-start fixtures do not spin"
        );
        assert!(ensure::should_retry_idle_replace(1, Some(1)));
        assert!(!ensure::should_retry_idle_replace(2, Some(1)));
    }

    #[tokio::test]
    async fn wait_idle_replaces_after_a_bound_unhealthy_listener_stops_responding() {
        let root = tempfile::tempdir().expect("wait-idle defer-start fixture");
        let dummy = root.path().join("claudex-agent-adapter");
        std::fs::write(&dummy, "#!/bin/sh\nexit 0\n").expect("dummy adapter");
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&dummy)
                .expect("dummy metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&dummy, permissions).expect("dummy executable");
        }
        let mut cfg = config();
        cfg.executable = dummy;
        let listener = TcpListener::bind("127.0.0.1:0").expect("unhealthy listener");
        cfg.options.listen = listener.local_addr().expect("unhealthy address");
        cfg.log_path = root.path().join("adapter.log");
        cfg.lock_path = root.path().join("adapter.lock");
        let server = serve_responses(
            listener,
            vec![http_response("500 Internal Server Error", "unhealthy")],
        );
        let error = ensure::run(&cfg, ensure::Mode::WaitIdle)
            .await
            .expect_err("unhealthy bound listener should fall through to start");
        assert!(!error.to_string().is_empty());
        server.join().expect("unhealthy listener");
    }

    #[tokio::test]
    async fn wait_idle_start_reports_when_the_new_adapter_never_becomes_ready() {
        let root = tempfile::tempdir().expect("wait-idle start fixture");
        let dummy = root.path().join("claudex-agent-adapter");
        std::fs::write(&dummy, "#!/bin/sh\nexit 0\n").expect("dummy adapter");
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&dummy)
                .expect("dummy metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&dummy, permissions).expect("dummy executable");
        }
        let mut cfg = config();
        cfg.executable = dummy;
        cfg.options.listen = unused_listen();
        cfg.log_path = root.path().join("adapter.log");
        cfg.lock_path = root.path().join("adapter.lock");
        let error = ensure::run(&cfg, ensure::Mode::WaitIdle)
            .await
            .expect_err("unready start should fail");
        assert!(!error.to_string().is_empty());
    }

    #[tokio::test]
    async fn fallback_ignores_corrupt_state_and_still_attempts_a_new_listener() {
        let root = tempfile::tempdir().expect("fallback corrupt fixture");
        let dummy = root.path().join("claudex-agent-adapter");
        std::fs::write(&dummy, "#!/bin/sh\nexit 0\n").expect("dummy adapter");
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&dummy)
                .expect("dummy metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&dummy, permissions).expect("dummy executable");
        }
        let mut cfg = config();
        cfg.executable = dummy;
        cfg.options.listen = unused_listen();
        cfg.log_path = root.path().join("adapter.log");
        cfg.lock_path = root.path().join("adapter.lock");
        std::fs::write(
            root.path()
                .join(format!("fallback.{}.json", cfg.options.listen.port())),
            br#"{"listen":"not-a-socket","build_id":"b","service_config_fingerprint":"s","pid":1}"#,
        )
        .expect("corrupt fallback state");
        let error = fallback::ensure_current_generation(&reqwest::Client::new(), &cfg)
            .await
            .expect_err("corrupt state should not abort before start");
        assert!(
            !error
                .to_string()
                .contains("decode current-build fallback state"),
            "corrupt fallback state leaked into ensure: {error:#}"
        );
    }

    #[tokio::test]
    async fn fallback_start_reports_when_the_new_listener_never_becomes_ready() {
        let root = tempfile::tempdir().expect("fallback start fixture");
        let dummy = root.path().join("claudex-agent-adapter");
        std::fs::write(&dummy, "#!/bin/sh\nexit 0\n").expect("dummy adapter");
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&dummy)
                .expect("dummy metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&dummy, permissions).expect("dummy executable");
        }
        let mut cfg = config();
        cfg.executable = dummy;
        cfg.options.listen = unused_listen();
        cfg.log_path = root.path().join("adapter.log");
        cfg.lock_path = root.path().join("adapter.lock");
        let error = fallback::ensure_current_generation(&reqwest::Client::new(), &cfg)
            .await
            .expect_err("unready fallback should fail");
        assert!(!error.to_string().is_empty());
    }

    #[tokio::test]
    async fn after_update_failure_without_generation_keeps_the_original_error() {
        let error = recovery::after_update_failure(
            &reqwest::Client::new(),
            &config(),
            None,
            anyhow::anyhow!("start failed"),
        )
        .await
        .expect_err("missing generation stays failed");
        assert!(error.to_string().contains("start failed"));
    }

    #[tokio::test]
    async fn after_update_failure_with_a_missing_snapshot_keeps_the_update_error() {
        let error = recovery::after_update_failure(
            &reqwest::Client::new(),
            &config(),
            Some("missing-generation"),
            anyhow::anyhow!("start failed"),
        )
        .await
        .expect_err("missing snapshot stays failed");
        let message = format!("{error:#}");
        assert!(message.contains("start failed"));
        assert!(message.contains("recovery failed"));
    }

    #[test]
    fn reports_a_nonblocking_lock_error_for_an_invalid_file_descriptor() {
        let error = super::launcher_lock::try_lock_file_descriptor(-1)
            .expect_err("invalid file descriptor must fail");
        assert!(error.to_string().contains("try lock launcher state"));
    }

    #[tokio::test]
    async fn releases_a_matched_stale_listener_and_reports_a_deadline() {
        let client = reqwest::Client::new();
        let mut released = config();
        released.options.listen = unused_listen();
        let gracefully_terminated = Arc::new(AtomicBool::new(false));
        let gracefully_terminated_for_callback = Arc::clone(&gracefully_terminated);
        let force_terminated = Arc::new(AtomicBool::new(false));
        let force_terminated_for_callback = Arc::clone(&force_terminated);
        handover::release_stale_listener_with(
            &client,
            &released,
            Some(42),
            |pid, executable| pid == 42 && executable == Path::new("/tmp/adapter"),
            move |pid| mark_terminated(pid, &gracefully_terminated_for_callback),
            move |pid| mark_terminated(pid, &force_terminated_for_callback),
            Instant::now() + Duration::from_millis(40),
        )
        .await
        .expect("release stale listener");
        assert!(gracefully_terminated.load(Ordering::SeqCst));
        assert!(!force_terminated.load(Ordering::SeqCst));

        let listener = TcpListener::bind("127.0.0.1:0").expect("occupied listener");
        let mut occupied = config();
        occupied.options.listen = listener.local_addr().expect("occupied address");
        let accepting_listener = listener.try_clone().expect("clone occupied listener");
        accepting_listener
            .set_nonblocking(true)
            .expect("make health listener nonblocking");
        let stopped = Arc::new(AtomicBool::new(false));
        let server_stopped = Arc::clone(&stopped);
        let stale_health = health_response(&healthy(&occupied));
        let server = spawn_response_server(accepting_listener, stale_health, server_stopped);
        let gracefully_terminated = Arc::new(AtomicBool::new(false));
        let gracefully_terminated_for_callback = Arc::clone(&gracefully_terminated);
        let force_terminated = Arc::new(AtomicBool::new(false));
        let force_terminated_for_callback = Arc::clone(&force_terminated);
        let error = handover::release_stale_listener_with(
            &client,
            &occupied,
            Some(42),
            |pid, executable| pid == 42 && executable == Path::new("/tmp/adapter"),
            move |pid| mark_terminated(pid, &gracefully_terminated_for_callback),
            move |pid| mark_terminated(pid, &force_terminated_for_callback),
            Instant::now() + Duration::from_millis(40),
        )
        .await
        .expect_err("occupied stale listener must time out");
        assert!(error.to_string().contains("did not release its listener"));
        assert!(gracefully_terminated.load(Ordering::SeqCst));
        assert!(!force_terminated.load(Ordering::SeqCst));
        stopped.store(true, Ordering::SeqCst);
        server.join().expect("occupied listener server");
        drop(listener);
    }

    fn unused_listen() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("unused listener");
        listener.local_addr().expect("unused address")
    }

    fn mark_terminated(pid: u32, terminated: &AtomicBool) {
        assert_eq!(pid, 42);
        terminated.store(true, Ordering::SeqCst);
    }

    #[cfg(unix)]
    #[test]
    #[allow(clippy::zombie_processes)]
    fn daemon_terminate_uses_term_then_kill_for_resistant_process_groups() {
        let root = tempfile::tempdir().expect("terminate fixture");
        let child_pid_file = root.path().join("child.pid");
        let script = root.path().join("resistant-daemon.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 trap '' TERM\n\
                 sleep 100 &\n\
                 echo $! > '{}'\n\
                 while true; do sleep 1; done\n",
                child_pid_file.to_string_lossy()
            ),
        )
        .expect("daemon script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("daemon executable");
        let mut command = Command::new("sh");
        command.arg(script).process_group(0);
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("launch resistant daemon");
        let child_id = child.id();
        let _cleanup = ProcessGroupCleanup::for_leader(child_id);
        let child_pid = wait_for_child_pid(&child_pid_file);

        daemon_process::terminate(child_id);
        let stop_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let _status = wait_for_child_or_kill(&mut child, stop_deadline);

        assert!(!process_alive(child_id));
        assert!(!process_alive(child_pid));
    }

    #[cfg(unix)]
    fn wait_for_child_pid(path: &Path) -> u32 {
        for _ in 0..100 {
            match read_pid(path) {
                Some(pid) => return pid,
                None => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        panic!("child pid written")
    }

    #[cfg(unix)]
    fn read_pid(path: &Path) -> Option<u32> {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw_pid| raw_pid.trim().parse::<u32>().ok())
    }

    #[cfg(unix)]
    fn wait_for_child_or_kill(
        child: &mut std::process::Child,
        deadline: std::time::Instant,
    ) -> std::process::ExitStatus {
        while std::time::Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(status)) => return status,
                _ => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
        child.kill().expect("resistant daemon still running");
        child.wait().expect("daemon process terminated")
    }

    #[cfg(unix)]
    struct ProcessGroupCleanup(i32);

    #[cfg(unix)]
    impl ProcessGroupCleanup {
        fn for_leader(pid: u32) -> Self {
            Self(pid.try_into().expect("process group id fits in i32"))
        }
    }

    #[cfg(unix)]
    impl Drop for ProcessGroupCleanup {
        fn drop(&mut self) {
            let _result = unsafe { libc::kill(-self.0, libc::SIGKILL) };
        }
    }

    fn serve_response_until_stopped(
        listener: TcpListener,
        response: String,
        stopped: Arc<AtomicBool>,
    ) {
        while !stopped.load(Ordering::SeqCst) {
            serve_response_attempt(&listener, &response);
        }
    }

    fn serve_response_attempt(listener: &TcpListener, response: &str) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _result = stream.write_all(response.as_bytes());
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("accept health request: {error}"),
        }
    }

    fn spawn_response_server(
        listener: TcpListener,
        response: String,
        stopped: Arc<AtomicBool>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || serve_response_until_stopped(listener, response, stopped))
    }

    fn health_response(health: &Health) -> String {
        http_response(
            "200 OK",
            &json!({
                "status": health.status,
                "pid": health.pid,
                "protocol_version": health.protocol_version,
                "build_id": health.build_id,
                "model": health.model,
                "codex_config_fingerprint": health.codex_config_fingerprint,
                "service_config_fingerprint": health.service_config_fingerprint,
                "backend_routes": health.backend_routes,
                "worker_routes": health.worker_routes,
                "subscription_max_processes": health.subscription_max_processes,
                "subscription_timeout_minutes": health.subscription_timeout_minutes,
                "subagent_hard_timeout_seconds": health.subagent_hard_timeout_seconds,
                "recovery_generation": health.recovery_generation,
                "active_http_requests": health.active_http_requests,
                "active_provider_turns": health.active_provider_turns,
                "active_subagent_models": health.active_subagent_models,
                "listener_handover": health.listener_handover,
                "listen": health.listen,
                "active_claude_session_ids": health.active_claude_session_ids,
                "busy_claude_session_ids": health.busy_claude_session_ids,
            })
            .to_string(),
        )
    }

    #[cfg(unix)]
    fn process_alive(pid: u32) -> bool {
        let output = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "stat="])
            .output()
            .ok();
        let Some(output) = output else {
            return false;
        };
        let output_state = String::from_utf8_lossy(&output.stdout);
        let Some(state) = output_state.split_whitespace().next() else {
            return false;
        };
        if state.starts_with('Z') {
            return false;
        }
        output.status.success()
    }

    fn http_response(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn serve_responses(listener: TcpListener, responses: Vec<String>) -> thread::JoinHandle<()> {
        thread::spawn(move || serve_all_responses(&listener, responses))
    }

    fn serve_all_responses(listener: &TcpListener, responses: Vec<String>) {
        for response in responses {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0; 1_024];
            let bytes_read = stream.read(&mut request).expect("read request");
            assert!(bytes_read > 0, "request must contain bytes");
            stream
                .write_all(response.as_bytes())
                .expect("write response");
            stream.flush().expect("flush response");
        }
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

    #[cfg(unix)]
    #[test]
    fn starts_a_detached_daemon_with_the_configured_arguments() {
        let root = tempfile::tempdir().expect("daemon start fixture");
        let arguments = root.path().join("arguments");
        let executable = root.path().join("daemon.sh");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
                arguments.to_string_lossy()
            ),
        )
        .expect("daemon script");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("daemon executable");

        let mut config = config();
        config.executable = executable;
        config.log_path = root.path().join("adapter.log");
        let _pid = start_adapter(&config).expect("start detached daemon");

        assert!(
            wait_for_path(&arguments, 500, Duration::from_millis(10)),
            "detached daemon did not write arguments"
        );
        let arguments = std::fs::read_to_string(arguments).expect("daemon arguments");
        assert!(arguments.contains("serve\n"));
        assert!(arguments.contains("--model\ntest-model\n"));
        assert!(config.log_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn warm_start_recovery_snapshot_uses_the_canonical_listen() {
        let root = tempfile::tempdir().expect("warm-start recovery fixture");
        let executable = root.path().join("daemon.sh");
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("daemon script");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("daemon executable");
        let retained = root.path().join("retained.json");
        std::fs::write(&retained, "{}").expect("retained state");

        let mut canonical = config();
        canonical.executable = executable;
        canonical.log_path = root.path().join("adapter.log");
        let warm = canonical.with_listen("127.0.0.1:18318".parse().expect("warm listen"));

        super::daemon_start::start_adapter_with_retained(&warm, &retained, &canonical)
            .expect("warm-start with canonical recovery snapshot");

        let generation = super::recovery_manifest::generation_name(
            canonical.options.listen,
            env!("CLAUDEX_BUILD_ID"),
            &canonical.service_config_fingerprint,
        );
        super::recovery_manifest::validate(&canonical, &generation)
            .expect("canonical listen must validate after live-update warm-start");
        let mismatch = super::recovery_manifest::validate(&warm, &generation)
            .expect_err("ephemeral listen must not own the live-update snapshot");
        assert!(mismatch.to_string().contains("recovery listener mismatch"));
    }

    #[cfg(unix)]
    #[test]
    fn recovery_manifest_restarts_a_private_executable_snapshot() {
        let root = tempfile::tempdir().expect("recovery fixture");
        let executable = root.path().join("adapter-script");
        std::fs::write(
            &executable,
            "#!/bin/sh\ntrap 'exit 0' TERM INT\nwhile :; do sleep 1; done\n",
        )
        .expect("recovery executable");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .expect("recovery executable permissions");
        let mut config = config();
        config.executable = executable;
        config.log_path = root.path().join("adapter.log");
        let manifest =
            super::recovery_manifest::prepare(&config).expect("prepare recovery manifest");
        assert_eq!(
            std::fs::metadata(&manifest)
                .expect("manifest metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let generation =
            super::recovery_manifest::generation_from_path(&manifest).expect("manifest generation");
        let validated = super::recovery_manifest::validate(&config, &generation)
            .expect("validate private recovery generation");
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::symlink_metadata(&manifest).unwrap().uid(),
            unsafe { libc::geteuid() }
        );
        let recovery = super::daemon_start::start_recovery(&config, &generation)
            .expect("start recovery snapshot");
        assert!(process_alive(recovery.pid));
        daemon_process::terminate(recovery.pid);

        std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(super::recovery_manifest::validate(&config, &generation).is_err());
        std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::remove_file(&manifest).unwrap();
        std::os::unix::fs::symlink(&validated.executable, &manifest).unwrap();
        let error = super::recovery_manifest::validate(&config, &generation)
            .expect_err("symlinked recovery manifest must be rejected");
        assert!(error.to_string().contains("symlink"));
    }

    #[test]
    fn missing_recovery_snapshot_still_allows_preflight_migration() {
        let root = tempfile::tempdir().expect("missing recovery fixture");
        let mut cfg = config();
        cfg.log_path = root.path().join("adapter.log");
        assert_eq!(
            ensure::usable_recovery_generation(&cfg, None).expect("no generation"),
            None
        );
        assert_eq!(
            ensure::usable_recovery_generation(&cfg, Some("v1-missing-generation"))
                .expect("missing snapshot"),
            None,
            "pruned or deleted recovery snapshot must not block Replace"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_recovery_snapshot_still_blocks_handover() {
        let root = tempfile::tempdir().expect("unsafe recovery fixture");
        let dummy = root.path().join("claudex-agent-adapter");
        std::fs::write(&dummy, "#!/bin/sh\nexit 0\n").expect("dummy adapter");
        std::fs::set_permissions(&dummy, std::fs::Permissions::from_mode(0o755))
            .expect("dummy executable");
        let mut cfg = config();
        cfg.executable = dummy;
        cfg.log_path = root.path().join("adapter.log");
        let manifest = super::recovery_manifest::prepare(&cfg).expect("prepare recovery manifest");
        let generation =
            super::recovery_manifest::generation_from_path(&manifest).expect("manifest generation");
        std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o644))
            .expect("unsafe manifest permissions");
        let error = ensure::usable_recovery_generation(&cfg, Some(&generation))
            .expect_err("unsafe manifest must block handover");
        assert!(error.to_string().contains("recovery"));
    }

    #[test]
    fn recovery_generations_include_full_listener_build_and_configuration_identity() {
        let ipv4 = "127.0.0.1:8318".parse().unwrap();
        let ipv6 = "[::1]:8318".parse().unwrap();
        let base = super::recovery_manifest::generation_name(ipv4, "build-a", "config-a");
        assert_ne!(
            base,
            super::recovery_manifest::generation_name(ipv6, "build-a", "config-a")
        );
        assert_ne!(
            base,
            super::recovery_manifest::generation_name(ipv4, "build-b", "config-a")
        );
        assert_ne!(
            base,
            super::recovery_manifest::generation_name(ipv4, "build-a", "config-b")
        );
    }

    #[tokio::test]
    async fn readiness_waits_for_matching_authenticated_health() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("readiness listener");
        let mut ready_config = config();
        ready_config.options.listen = listener.local_addr().expect("readiness address");
        let ready_health = healthy(&ready_config);
        let mut stale = healthy(&ready_config);
        stale.status = "starting".to_owned();
        let server = serve_responses(
            listener,
            vec![
                health_response(&ready_health),
                http_response("401 Unauthorized", "{}"),
                health_response(&stale),
                health_response(&ready_health),
                http_response("200 OK", "{}"),
            ],
        );

        wait_until_ready_with(
            &reqwest::Client::new(),
            &ready_config,
            Duration::from_millis(300),
            Duration::from_millis(1),
            Duration::from_millis(2),
        )
        .await
        .expect("matching authenticated health becomes ready");
        server.join().expect("readiness server");
    }

    #[tokio::test]
    async fn readiness_retries_when_a_compatible_daemon_has_an_old_build() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("readiness listener");
        let mut ready_config = config();
        ready_config.options.listen = listener.local_addr().expect("readiness address");
        let mut old_build = healthy(&ready_config);
        old_build.build_id = "previous-build".to_owned();
        let server = serve_responses(
            listener,
            vec![
                health_response(&old_build),
                health_response(&healthy(&ready_config)),
                http_response("200 OK", "{}"),
            ],
        );

        wait_until_ready_with(
            &reqwest::Client::new(),
            &ready_config,
            Duration::from_millis(100),
            Duration::from_millis(1),
            Duration::from_millis(2),
        )
        .await
        .expect("current build becomes ready");
        server.join().expect("readiness server");
    }

    #[tokio::test]
    async fn handover_ignores_unmatched_processes_and_waits_for_stale_health() {
        let client = reqwest::Client::new();
        let ignored = Arc::new(AtomicBool::new(false));
        let ignored_graceful = Arc::clone(&ignored);
        let ignored_force = Arc::clone(&ignored);
        handover::release_stale_listener_with(
            &client,
            &config(),
            Some(42),
            |_pid, _executable| false,
            move |_pid| ignored_graceful.store(true, Ordering::SeqCst),
            move |_pid| ignored_force.store(true, Ordering::SeqCst),
            Instant::now() + Duration::from_millis(20),
        )
        .await
        .expect("unmatched process is ignored");
        assert!(!ignored.load(Ordering::SeqCst));

        let listener = TcpListener::bind("127.0.0.1:0").expect("stale health listener");
        let mut released = config();
        released.options.listen = listener.local_addr().expect("stale health address");
        let mut stale_health = healthy(&released);
        stale_health.pid = Some(42);
        let server = serve_responses(listener, vec![health_response(&stale_health)]);
        let graceful = Arc::new(AtomicBool::new(false));
        let graceful_callback = Arc::clone(&graceful);
        let forced = Arc::new(AtomicBool::new(false));
        let forced_callback = Arc::clone(&forced);
        handover::release_stale_listener_with(
            &client,
            &released,
            Some(42),
            |pid, executable| pid == 42 && executable == Path::new("/tmp/adapter"),
            move |pid| mark_terminated(pid, &graceful_callback),
            move |pid| mark_terminated(pid, &forced_callback),
            Instant::now() + Duration::from_secs(1),
        )
        .await
        .expect("stale listener releases after its health response");
        assert!(graceful.load(Ordering::SeqCst));
        assert!(!forced.load(Ordering::SeqCst));
        server.join().expect("stale health server");
    }

    #[test]
    fn keeps_non_file_logs_and_archives_extensionless_logs() {
        let root = tempfile::tempdir().expect("log fixture");
        let directory = root.path().join("directory");
        std::fs::create_dir(&directory).expect("log directory");
        super::launcher_logs::archive_previous_log(&directory).expect("directory is not a log");
        assert!(directory.is_dir());

        let extensionless = root.path().join("adapter");
        std::fs::write(&extensionless, "old").expect("extensionless log");
        super::launcher_logs::archive_previous_log(&extensionless)
            .expect("archive extensionless log");
        assert!(!extensionless.exists());
        assert_eq!(
            std::fs::read_dir(root.path())
                .expect("log directory")
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_file())
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn graceful_shutdown_signals_a_live_daemon_but_not_unsafe_pids() {
        let root = tempfile::tempdir().expect("graceful shutdown fixture");
        let ready = root.path().join("ready");
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "trap 'exit 0' TERM; : > \"$1\"; while :; do :; done",
                "sh",
                ready.to_str().expect("ready path"),
            ])
            .process_group(0);
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start graceful shutdown fixture");
        let _cleanup = ProcessGroupCleanup::for_leader(child.id());
        assert!(
            wait_for_path(&ready, 100, Duration::from_millis(1)),
            "fixture installed its TERM handler"
        );

        daemon_process::request_graceful_shutdown(child.id());
        assert!(child.wait().expect("reap graceful fixture").success());
        daemon_process::request_graceful_shutdown(0);
        daemon_process::request_graceful_shutdown(u32::MAX);
        daemon_process::request_graceful_shutdown(std::process::id());
    }

    #[cfg(unix)]
    #[test]
    #[allow(clippy::zombie_processes)]
    fn daemon_terminate_kills_an_orphaned_term_ignoring_process_group() {
        let root = tempfile::tempdir().expect("orphaned daemon fixture");
        let child_pid_file = root.path().join("child.pid");
        let script = root.path().join("orphaned-daemon.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 trap '' TERM\n\
                 sleep 100 &\n\
                 echo $! > '{}'\n\
                 exit 0\n",
                child_pid_file.to_string_lossy()
            ),
        )
        .expect("orphaned daemon script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("orphaned daemon executable");
        let mut command = Command::new("sh");
        command.arg(script).process_group(0);
        let mut parent = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("launch orphaned daemon");
        let parent_pid = parent.id();
        let _cleanup = ProcessGroupCleanup::for_leader(parent_pid);
        let child_pid = wait_for_child_pid(&child_pid_file);
        assert!(parent.wait().expect("reap exited daemon").success());

        daemon_process::terminate(parent_pid);
        assert!(wait_for_process_stop(child_pid));
        daemon_process::terminate(std::process::id());
    }

    fn wait_for_path(path: &Path, attempts: usize, delay: Duration) -> bool {
        for _ in 0..attempts {
            match path.exists() {
                true => return true,
                false => thread::sleep(delay),
            }
        }
        false
    }

    #[cfg(unix)]
    fn wait_for_process_stop(pid: u32) -> bool {
        for _ in 0..100 {
            match process_alive(pid) {
                false => return true,
                true => thread::sleep(Duration::from_millis(10)),
            }
        }
        false
    }
}
