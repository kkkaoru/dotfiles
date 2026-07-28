#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::{
        io::{Read, Write},
        net::{SocketAddr, TcpListener},
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Instant,
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
                model_catalog: crate::provider_config::ModelCatalog::default(),
            },
            token: LOCAL_TOKEN.to_owned(),
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
    fn acquires_launcher_lock_and_rejects_a_parentless_path() {
        let root = tempfile::tempdir().expect("lock fixture");
        let lock_path = root.path().join("adapter.lock");
        let guard = super::launcher_lock::acquire(&lock_path).expect("lock acquisition");
        assert!(lock_path.exists());
        drop(guard);
        assert!(super::launcher_lock::acquire(Path::new("")).is_err());
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

    fn healthy(config: &ServiceConfig) -> Health {
        Health {
            status: "ok".to_owned(),
            pid: Some(42),
            protocol_version: ADAPTER_PROTOCOL_VERSION,
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
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
    async fn inspects_start_reuse_and_replacement_service_states() {
        let client = reqwest::Client::new();
        let mut absent = config();
        absent.options.listen = unused_listen();
        assert_eq!(
            handover::inspect_service(&client, &absent).await,
            handover::ServiceState::Start
        );

        let mut reusable = config();
        let listener = TcpListener::bind("127.0.0.1:0").expect("reuse listener");
        reusable.options.listen = listener.local_addr().expect("reuse address");
        let health = healthy(&reusable);
        let server = serve_responses(
            listener,
            vec![health_response(&health), http_response("200 OK", "{}")],
        );
        assert_eq!(
            handover::inspect_service(&client, &reusable).await,
            handover::ServiceState::Reuse
        );
        server.join().expect("reuse server");

        let mut stale = healthy(&reusable);
        stale.build_id = "old-build".to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").expect("stale listener");
        reusable.options.listen = listener.local_addr().expect("stale address");
        let server = serve_responses(listener, vec![health_response(&stale)]);
        assert_eq!(
            handover::inspect_service(&client, &reusable).await,
            handover::ServiceState::Replace(Some(42))
        );
        server.join().expect("stale server");

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
            handover::inspect_service(&client, &reusable).await,
            handover::ServiceState::Replace(Some(42))
        );
        server.join().expect("authentication server");
    }

    #[tokio::test]
    async fn releases_a_matched_stale_listener_and_reports_a_deadline() {
        let client = reqwest::Client::new();
        let mut released = config();
        released.options.listen = unused_listen();
        let terminated = Arc::new(AtomicBool::new(false));
        let terminated_for_callback = Arc::clone(&terminated);
        handover::release_stale_listener_with(
            &client,
            &released,
            Some(42),
            |pid, executable| pid == 42 && executable == Path::new("/tmp/adapter"),
            move |pid| mark_terminated(pid, &terminated_for_callback),
        )
        .await
        .expect("release stale listener");
        assert!(terminated.load(Ordering::SeqCst));

        let listener = TcpListener::bind("127.0.0.1:0").expect("occupied listener");
        let mut occupied = config();
        occupied.options.listen = listener.local_addr().expect("occupied address");
        let accepting_listener = listener.try_clone().expect("clone occupied listener");
        accepting_listener
            .set_nonblocking(true)
            .expect("make health listener nonblocking");
        let stopped = Arc::new(AtomicBool::new(false));
        let server_stopped = Arc::clone(&stopped);
        let server = thread::spawn(move || accept_until_stopped(accepting_listener, server_stopped));
        let error = handover::wait_until_listener_released_by(
            &client,
            &occupied,
            42,
            Instant::now() + Duration::from_millis(40),
        )
        .await
        .expect_err("occupied stale listener must time out");
        assert!(error.to_string().contains("did not release its listener"));
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

    fn accept_until_stopped(listener: TcpListener, stopped: Arc<AtomicBool>) {
        while !stopped.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((_stream, _)) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("accept health request: {error}"),
            }
        }
    }

    fn health_response(health: &Health) -> String {
        http_response(
            "200 OK",
            &json!({
                "status": health.status,
                "pid": health.pid,
                "protocol_version": health.protocol_version,
                "build_id": health.build_id,
                "backend_routes": health.backend_routes,
                "subscription_max_processes": health.subscription_max_processes,
                "subscription_timeout_minutes": health.subscription_timeout_minutes,
            })
            .to_string(),
        )
    }

    fn http_response(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn serve_responses(listener: TcpListener, responses: Vec<String>) -> thread::JoinHandle<()> {
        thread::spawn(move || {
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
        })
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
