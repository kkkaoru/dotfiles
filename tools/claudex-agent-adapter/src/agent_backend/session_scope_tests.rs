    use super::*;
    use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};
    use std::sync::Arc;

    #[test]
    fn scope_key_uses_anonymous_for_missing_ids() {
        assert_eq!(
            SessionScopedBackends::scope_key(None),
            ANONYMOUS_SESSION_SCOPE
        );
        assert_eq!(
            SessionScopedBackends::scope_key(Some("")),
            ANONYMOUS_SESSION_SCOPE
        );
        assert_eq!(SessionScopedBackends::scope_key(Some("sess-a")), "sess-a");
    }

    #[test]
    fn distinct_claude_sessions_get_independent_routed_pools() {
        let scopes =
            SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
        let a = scopes.scope(Some("session-a"));
        let b = scopes.scope(Some("session-b"));
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(scopes.scope_count(), 2);
        assert!(Arc::ptr_eq(&a, &scopes.scope(Some("session-a"))));
    }

    #[test]
    fn concurrent_scope_lookup_reuses_one_pool_per_session_without_crossing_sessions() {
        let scopes = Arc::new(SessionScopedBackends::new(&[
            BackendRoute::new("main", BackendKind::CodexAppServer),
        ]));
        let mut workers = Vec::new();
        for index in 0..32 {
            let scopes = Arc::clone(&scopes);
            workers.push(std::thread::spawn(move || {
                let id = if index % 2 == 0 {
                    "parallel-a"
                } else {
                    "parallel-b"
                };
                Arc::as_ptr(&scopes.scope(Some(id))) as usize
            }));
        }
        let addresses = workers
            .into_iter()
            .map(|worker| worker.join().expect("scope worker"))
            .collect::<Vec<_>>();
        let a = scopes.scope(Some("parallel-a"));
        let b = scopes.scope(Some("parallel-b"));
        assert_eq!(scopes.scope_count(), 2);
        assert!(addresses.iter().enumerate().all(|(index, address)| {
            *address == Arc::as_ptr(if index % 2 == 0 { &a } else { &b }) as usize
        }));
        assert!(!Arc::ptr_eq(&a, &b));
    }

    #[tokio::test]
    async fn release_scope_drops_the_pool() {
        let scopes =
            SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
        let _ = scopes.scope(Some("session-a"));
        assert_eq!(scopes.scope_count(), 1);
        scopes.release_scope(Some("session-a")).await;
        assert_eq!(scopes.scope_count(), 0);
    }

    #[tokio::test]
    async fn release_scope_waits_for_leaf_shutdown_before_scope_is_gone() {
        let scopes = SessionScopedBackends::new(&[BackendRoute::new(
            "main",
            BackendKind::GrokAcp,
        )]);
        let leaf = Arc::new(AgentBackend::Grok(
            crate::grok_acp::GrokAcp::alive_for_test(),
        ));
        scopes.insert_scope_for_test(
            "shutdown-order",
            AgentBackend::routed(vec![("main".to_owned(), Arc::clone(&leaf))]),
        );

        scopes.release_scope(Some("shutdown-order")).await;

        assert!(!leaf.is_alive(), "scope release must await leaf cleanup");
        assert_eq!(scopes.scope_count(), 0);
    }

    #[test]
    fn empty_scopes_report_models_alive_and_catalog_metadata() {
        let scopes =
            SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
        assert!(scopes.model_is_alive("main"));
        assert!(scopes.started_models().is_empty());
        assert!(scopes.catalog().supports("main"));
    }

    #[tokio::test]
    async fn shutdown_all_clears_every_scope() {
        let scopes =
            SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
        let _ = scopes.scope(Some("a"));
        let _ = scopes.scope(Some("b"));
        assert_eq!(scopes.scope_count(), 2);
        scopes.shutdown_all().await;
        assert_eq!(scopes.scope_count(), 0);
    }

    #[test]
    fn scope_snapshots_sort_and_report_started_models() {
        let scopes =
            SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
        let _ = scopes.scope(Some("sess-b"));
        let _ = scopes.scope(Some("sess-a"));
        let snapshots = scopes.scope_snapshots();
        assert_eq!(
            snapshots
                .iter()
                .map(|snapshot| snapshot.claude_session_id.as_str())
                .collect::<Vec<_>>(),
            ["sess-a", "sess-b"]
        );
        assert!(
            snapshots
                .iter()
                .all(|snapshot| snapshot.started_models.is_empty())
        );
    }

    #[test]
    fn unique_started_pool_is_none_for_lazy_scopes() {
        let scopes = SessionScopedBackends::new(&[BackendRoute::new(
            "glm-5.2:cloud",
            BackendKind::CodexAppServer,
        )]);
        let _ = scopes.scope(Some("tui-session"));
        assert!(
            scopes
                .unique_started_pool_for_model("glm-5.2:cloud")
                .is_none()
        );
    }

    #[test]
    fn scope_or_self_clones_non_scoped_backends() {
        let leaf = AgentBackend::spawn_routes(&[]);
        let scoped = leaf.scope_or_self(Some("sess"));
        assert!(!Arc::ptr_eq(&leaf, &scoped));
        let codex = AgentBackend::routed(Vec::new());
        assert!(Arc::ptr_eq(&codex, &codex.scope_or_self(Some("sess"))));
    }

    #[test]
    fn scope_create_and_reuse_emit_structured_log_events() {
        use tracing::level_filters::LevelFilter;

        let buffer = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        struct BufferWriter(Arc<std::sync::Mutex<Vec<u8>>>);
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufferWriter {
            type Writer = BufferWriter;

            fn make_writer(&'a self) -> Self::Writer {
                BufferWriter(Arc::clone(&self.0))
            }
        }
        impl std::io::Write for BufferWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("log buffer").extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(LevelFilter::DEBUG)
            .with_writer(BufferWriter(Arc::clone(&buffer)))
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let scopes =
            SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
        let _ = scopes.scope(Some("log-sess"));
        let _ = scopes.scope(Some("log-sess"));
        let text = String::from_utf8(buffer.lock().expect("log buffer").clone()).unwrap();
        assert!(
            text.contains("provider_session_scope_create"),
            "missing create event in logs: {text}"
        );
        assert!(
            text.contains("provider_session_scope_reuse"),
            "missing reuse event in logs: {text}"
        );
        assert!(
            text.contains("log-sess"),
            "missing session id in logs: {text}"
        );
    }

    #[tokio::test]
    async fn scope_release_emits_structured_log_event() {
        use tracing::level_filters::LevelFilter;

        let buffer = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        struct BufferWriter(Arc<std::sync::Mutex<Vec<u8>>>);
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufferWriter {
            type Writer = BufferWriter;

            fn make_writer(&'a self) -> Self::Writer {
                BufferWriter(Arc::clone(&self.0))
            }
        }
        impl std::io::Write for BufferWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("log buffer").extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(LevelFilter::INFO)
            .with_writer(BufferWriter(Arc::clone(&buffer)))
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let scopes =
            SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
        let _ = scopes.scope(Some("release-sess"));
        scopes.release_scope(Some("release-sess")).await;
        let text = String::from_utf8(buffer.lock().expect("log buffer").clone()).unwrap();
        assert!(
            text.contains("provider_session_scope_release"),
            "missing release event in logs: {text}"
        );
        assert!(
            text.contains("release-sess"),
            "missing session id in logs: {text}"
        );
    }
