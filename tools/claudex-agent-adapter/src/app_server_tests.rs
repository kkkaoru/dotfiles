#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn extracts_or_rejects_thread_ids() {
        assert_eq!(
            response_thread_id(&json!({"thread":{"id":"thread-1"}})).unwrap(),
            "thread-1"
        );
        assert!(response_thread_id(&json!({"thread":{}})).is_err());
    }

    #[test]
    fn isolated_home_requires_authentication() {
        let root = tempfile::tempdir().unwrap();
        let error = prepare_isolated_codex_home(
            &root.path().join("missing"),
            &root.path().join("isolated"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("codex login"));
    }

    #[test]
    fn prepares_an_isolated_home_with_only_required_configuration() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let isolated = root.path().join("isolated");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("auth.json"), r#"{"token":"test"}"#).unwrap();
        std::fs::write(
            source.join("config.toml"),
            r#"[model_providers]
root = true

[model_providers.sakana]
name = "Sakana"
base_url = "https://api.sakana.ai/v1"
env_key = "SAKANA_AI_PRO_API_KEY"
wire_api = "responses"

[mcp_servers.must_not_copy]
command = "false"
"#,
        )
        .unwrap();
        std::fs::write(
            source.join("ollama.config.toml"),
            r#"[model_providers.ollama]
name = "Ollama"
base_url = "http://127.0.0.1:11434/v1"
wire_api = "responses"

[mcp_servers.must_not_copy]
command = "false"
"#,
        )
        .unwrap();
        std::fs::write(
            source.join("duplicate.config.toml"),
            r#"[model_providers.sakana]
name = "Must not replace the base config"
"#,
        )
        .unwrap();

        let prepared = prepare_isolated_codex_home(&source, &isolated).unwrap();
        assert_eq!(prepared, isolated);
        assert_eq!(
            std::fs::read_to_string(prepared.join("auth.json")).unwrap(),
            r#"{"token":"test"}"#
        );
        let config = std::fs::read_to_string(prepared.join("config.toml")).unwrap();
        assert!(config.contains("tool_search = false"));
        assert!(config.contains("plugins = false"));
        assert!(config.contains("[model_providers.sakana]"));
        assert!(config.contains("[model_providers.ollama]"));
        assert_eq!(config.matches("[model_providers.sakana]").count(), 1);
        assert!(!config.contains("Must not replace"));
        assert!(!config.contains("mcp_servers.must_not_copy"));
    }

    #[test]
    fn reports_an_unwritable_isolated_configuration() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let isolated = root.path().join("isolated");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&isolated).unwrap();
        std::fs::write(source.join("auth.json"), "{}").unwrap();
        std::fs::create_dir(isolated.join("config.toml")).unwrap();

        assert!(prepare_isolated_codex_home(&source, &isolated).is_err());
    }

    #[tokio::test]
    async fn reports_spawn_and_isolated_home_filesystem_failures() {
        let root = tempfile::tempdir().expect("app-server fixture");
        let source = root.path().join("source");
        std::fs::create_dir(&source).expect("source home");
        std::fs::write(source.join("auth.json"), "{}").expect("auth");

        let isolated_file = root.path().join("isolated-file");
        std::fs::write(&isolated_file, "occupied").expect("occupied isolated path");
        assert!(prepare_isolated_codex_home(&source, &isolated_file).is_err());

        let copy_failure = root.path().join("copy-failure");
        std::fs::create_dir(&copy_failure).expect("isolated home");
        std::fs::create_dir(copy_failure.join("auth.json")).expect("occupied auth target");
        assert!(prepare_isolated_codex_home(&source, &copy_failure).is_err());

        let error = AppServer::spawn_with_program(
            "model",
            root.path().join("missing-program"),
            &source,
            &root.path().join("spawn-home"),
        )
        .await
        .err()
        .expect("missing app-server program");
        assert!(error.to_string().contains("failed to start"));
    }

    #[tokio::test]
    async fn reports_initialize_failure_and_request_timeout() {
        let root = tempfile::tempdir().expect("create app-server fixture");
        let source = root.path().join("source");
        std::fs::create_dir(&source).expect("create source home");
        std::fs::write(source.join("auth.json"), "{}").expect("write auth");

        let failing = script(
            root.path(),
            "failing",
            "read line\nprintf '%s\\n' '{\"id\":1,\"error\":{\"message\":\"init failed\"}}'\n",
        );
        let error =
            AppServer::spawn_with_program("model", &failing, &source, &root.path().join("failed"))
                .await
                .err()
                .expect("initialize must fail");
        assert!(error.to_string().contains("initialization failed"));

        let stalled = script(
            root.path(),
            "stalled-program",
            "read line\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nwhile read line; do :; done\n",
        );
        let server = AppServer::spawn_with_program(
            "model",
            &stalled,
            &source,
            &root.path().join("stalled-home"),
        )
        .await
        .expect("start stalled server");
        let error = server
            .request_with_timeout("never/respond", json!({}), Duration::from_millis(5))
            .await
            .expect_err("request must time out");
        assert!(error.to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn stores_detached_requests_without_a_response_channel() {
        let root = tempfile::tempdir().expect("create app-server fixture");
        let source = root.path().join("source");
        std::fs::create_dir(&source).expect("create source home");
        std::fs::write(source.join("auth.json"), "{}").expect("write auth");
        let stalled = script(
            root.path(),
            "detached-program",
            "read line\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nwhile read line; do :; done\n",
        );
        let server = AppServer::spawn_with_program(
            "model",
            &stalled,
            &source,
            &root.path().join("detached-home"),
        )
        .await
        .expect("start stalled server");

        server
            .request_detached("turn/start", json!({"threadId":"thread"}))
            .await
            .expect("flush detached request");
        let pending = server.pending.lock().await;
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending.values().next(),
            Some(PendingResponse::Detached { thread_id }) if thread_id == "thread"
        ));
        drop(pending);
        server.shutdown().await;
        assert!(
            server
                .child
                .lock()
                .await
                .try_wait()
                .expect("inspect stopped app-server")
                .is_some(),
            "shutdown must reap the direct app-server child"
        );
        assert!(server
            .request_detached("turn/start", json!({"threadId":"after-stop"}))
            .await
            .is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_reaps_a_completed_parent_and_its_process_group() {
        let root = tempfile::tempdir().expect("app-server process-group fixture");
        let source = source_home(root.path());
        let program = script(
            root.path(),
            "completed-parent-program",
            "read initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nsleep 30 &\n",
        );
        let server = AppServer::spawn_with_program(
            "model",
            &program,
            &source,
            &root.path().join("completed-parent-home"),
        )
        .await
        .expect("start app-server fixture");
        let process_group = server
            .child
            .lock()
            .await
            .id()
            .expect("app-server process group ID");

        tokio::time::sleep(Duration::from_millis(20)).await;
        server.stop("completed parent test").await;

        assert!(server
            .child
            .lock()
            .await
            .try_wait()
            .expect("inspect completed app-server")
            .is_some());
        assert!(!process_group_exists(process_group));
    }

    #[tokio::test]
    async fn ignores_stop_requests_for_a_dropped_server() {
        let dropped = std::sync::Weak::<AppServer>::new();

        lifecycle::stop_if_alive(&dropped, "already dropped").await;

        assert!(dropped.upgrade().is_none());
    }

    #[tokio::test]
    async fn stop_is_idempotent() {
        let root = tempfile::tempdir().expect("app-server lifecycle fixture");
        let source = source_home(root.path());
        let program = script(
            root.path(),
            "idempotent-program",
            "read line\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nwhile read line; do :; done\n",
        );
        let server = AppServer::spawn_with_program(
            "model",
            &program,
            &source,
            &root.path().join("idempotent-home"),
        )
        .await
        .expect("start app-server fixture");
        server.stop("first stop").await;
        server.stop("second stop").await;
        assert!(!server.is_alive());
    }

    #[tokio::test]
    async fn writes_parallel_requests_without_head_of_line_blocking() {
        const INITIALIZE_REQUEST_ID: u64 = 1;
        const FIRST_REQUEST_ID: u64 = 2;
        const SECOND_REQUEST_ID: u64 = 3;
        const PARALLEL_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
        let root = tempfile::tempdir().expect("parallel app-server fixture");
        let source = root.path().join("source");
        std::fs::create_dir(&source).expect("create source home");
        std::fs::write(source.join("auth.json"), "{}").expect("write auth");
        let body = format!(
            "read initialize\nprintf '%s\\n' '{{\"id\":{INITIALIZE_REQUEST_ID},\"result\":{{}}}}'\n\
             read initialized\nread first_request\nread second_request\n\
             printf '%s\\n' '{{\"id\":{FIRST_REQUEST_ID},\"result\":{{\"thread\":{{\"id\":\"first\"}}}}}}'\n\
             printf '%s\\n' '{{\"id\":{SECOND_REQUEST_ID},\"result\":{{\"thread\":{{\"id\":\"second\"}}}}}}'\n\
             while read line; do :; done\n"
        );
        let program = script(root.path(), "parallel-program", &body);
        let server = AppServer::spawn_with_program(
            "model",
            &program,
            &source,
            &root.path().join("parallel-home"),
        )
        .await
        .expect("start parallel app-server");

        let (first, second) = tokio::time::timeout(PARALLEL_REQUEST_TIMEOUT, async {
            tokio::join!(
                server.request("thread/start", json!({"request":"first"})),
                server.request("thread/start", json!({"request":"second"}))
            )
        })
        .await
        .expect("parallel requests were serialized");

        assert_eq!(
            response_thread_id(&first.expect("first response")).unwrap(),
            "first"
        );
        assert_eq!(
            response_thread_id(&second.expect("second response")).unwrap(),
            "second"
        );
        server.stop("parallel request test complete").await;
    }

    fn script(root: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).expect("write script");
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make script executable");
        path
    }

    fn source_home(root: &std::path::Path) -> PathBuf {
        let source = root.join("source");
        std::fs::create_dir(&source).expect("create source home");
        std::fs::write(source.join("auth.json"), "{}").expect("write auth");
        source
    }

    #[cfg(unix)]
    fn process_group_exists(process_group: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &format!("-{process_group}")])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("inspect app-server process group")
            .success()
    }
}
