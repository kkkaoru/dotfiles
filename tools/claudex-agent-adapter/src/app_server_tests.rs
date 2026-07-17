#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
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

        let prepared = prepare_isolated_codex_home(&source, &isolated).unwrap();
        assert_eq!(prepared, isolated);
        assert_eq!(
            std::fs::read_to_string(prepared.join("auth.json")).unwrap(),
            r#"{"token":"test"}"#
        );
        let config = std::fs::read_to_string(prepared.join("config.toml")).unwrap();
        assert!(config.contains("tool_search = false"));
        assert!(config.contains("plugins = false"));
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

    fn script(root: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).expect("write script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make script executable");
        path
    }
}
