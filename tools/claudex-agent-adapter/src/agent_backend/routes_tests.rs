#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;

    use super::{MAX_DYNAMIC_ROUTES, RoutedBackends, StartupState, startup};
    use crate::agent_backend::{AcpLaunch, AgentBackend, BackendKind, BackendRoute, WebSearchMode};

    #[test]
    fn shares_codex_startup_but_keeps_acp_servers_model_specific() {
        let routes = RoutedBackends::lazy(&[
            route_with_prefix("codex-one", BackendKind::CodexAppServer, "codex-"),
            route_with_prefix("codex-two", BackendKind::CodexAppServer, "codex-"),
            route_with_prefix("copilot-one", BackendKind::CopilotAcp, "copilot-"),
            route_with_prefix("copilot-two", BackendKind::CopilotAcp, "copilot-"),
            route_with_prefix("acp-one", BackendKind::GrokAcp, "acp-"),
            route_with_prefix("acp-two", BackendKind::GrokAcp, "acp-"),
        ]);
        let codex_one = routes.route(0);
        let codex_two = routes.route(1);
        let copilot_one = routes.route(2);
        let copilot_two = routes.route(3);
        let acp_one = routes.route(4);
        let acp_two = routes.route(5);

        assert!(Arc::ptr_eq(&codex_one.startup, &codex_two.startup));
        assert!(!Arc::ptr_eq(&copilot_one.startup, &copilot_two.startup));
        assert!(!Arc::ptr_eq(&acp_one.startup, &acp_two.startup));

        let (_, dynamic_codex) = routes.resolve("codex-dynamic").unwrap();
        let (_, dynamic_acp) = routes.resolve("acp-dynamic").unwrap();
        assert!(Arc::ptr_eq(&codex_one.startup, &dynamic_codex.startup));
        assert!(!Arc::ptr_eq(&acp_one.startup, &dynamic_acp.startup));
    }

    #[test]
    fn shares_a_session_scoped_configured_child_only_for_identical_launches() {
        let mut first = route("vendor-one", BackendKind::ConfiguredAcp);
        first.model_prefixes.push("vendor-session-".to_owned());
        first.acp = Some(AcpLaunch {
            program: "provider".to_owned(),
            arguments: vec!["acp".to_owned()],
        });
        let mut second = first.clone();
        second.model = "vendor-two".to_owned();
        let mut launch_scoped = first.clone();
        launch_scoped.model = "vendor-launch".to_owned();
        launch_scoped.model_prefixes.clear();
        launch_scoped.acp = Some(AcpLaunch {
            program: "provider".to_owned(),
            arguments: vec!["--model".to_owned(), "{model}".to_owned()],
        });
        let mut different = first.clone();
        different.model = "vendor-different".to_owned();
        different.model_prefixes.clear();
        different.acp = Some(AcpLaunch {
            program: "other-provider".to_owned(),
            arguments: vec!["acp".to_owned()],
        });
        let routes = RoutedBackends::lazy(&[first, second, launch_scoped, different]);

        assert!(Arc::ptr_eq(
            &routes.route(0).startup,
            &routes.route(1).startup
        ));
        assert!(!Arc::ptr_eq(
            &routes.route(0).startup,
            &routes.route(2).startup
        ));
        assert!(!Arc::ptr_eq(
            &routes.route(0).startup,
            &routes.route(3).startup
        ));

        let (_, dynamic) = routes.resolve("vendor-session-dynamic").unwrap();
        assert!(Arc::ptr_eq(&routes.route(0).startup, &dynamic.startup));
    }

    #[test]
    fn bounds_dynamic_routes_but_reuses_existing_models() {
        let routes = RoutedBackends::lazy(&[route_with_prefix(
            "dynamic-base",
            BackendKind::CodexAppServer,
            "dynamic-",
        )]);
        for index in 0..MAX_DYNAMIC_ROUTES {
            let (route_index, route) = routes
                .resolve(&format!("dynamic-extra-{index}"))
                .expect("available dynamic route");
            assert_eq!(route_index, index + 1);
            assert_eq!(route.model, format!("dynamic-extra-{index}"));
        }
        let (existing, _) = routes.resolve("dynamic-extra-0").expect("existing route");
        assert_eq!(existing, 1);
        assert_eq!(routes.route(existing).model, "dynamic-extra-0");
        assert_eq!(
            routes.find("dynamic-extra-0").unwrap().model,
            "dynamic-extra-0"
        );
        assert!(routes.find("missing").is_none());
        assert!(routes.first_ready(BackendKind::CodexAppServer).is_none());
        assert!(routes.resolve("unconfigured-over-limit").is_err());
    }

    #[test]
    fn configured_prefixes_select_the_most_specific_backend() {
        let mut broad = route("broad", BackendKind::GrokAcp);
        broad.model_prefixes.push("vendor-".to_owned());
        let mut specific = route("specific", BackendKind::CopilotAcp);
        specific.model_prefixes.push("vendor-code-".to_owned());
        let routes = RoutedBackends::lazy(&[broad, specific]);

        assert!(routes.supports("vendor-code-new"));
        let (_, selected) = routes.resolve("vendor-code-new").unwrap();
        assert_eq!(selected.kind, BackendKind::CopilotAcp);
        assert_eq!(selected.model, "vendor-code-new");
        assert!(routes.supports("vendor-chat-new"));
        assert_eq!(
            routes.resolve("vendor-chat-new").unwrap().1.kind,
            BackendKind::GrokAcp
        );
        assert!(routes.first_ready(BackendKind::CodexAppServer).is_none());
        assert!(!routes.supports("unconfigured-model"));
        assert!(routes.resolve("unconfigured-model").is_err());
    }

    #[test]
    fn concurrency_limits_follow_the_exact_or_most_specific_prefix() {
        let mut broad = route("vendor-default", BackendKind::GrokAcp);
        broad.max_concurrency = Some(3);
        broad.model_prefixes.push("vendor-".to_owned());
        let mut specific = route("vendor-code", BackendKind::GrokAcp);
        specific.max_concurrency = Some(7);
        specific.model_prefixes.push("vendor-code-".to_owned());
        let routes = RoutedBackends::lazy(&[broad, specific]);

        assert_eq!(routes.max_concurrency_for_model("vendor-chat"), Some(3));
        assert_eq!(
            routes.max_concurrency_for_model("vendor-code-next"),
            Some(7)
        );
        assert_eq!(routes.max_concurrency_for_model("vendor-default"), Some(3));
        assert_eq!(routes.max_concurrency_for_model("other"), None);
        assert_eq!(
            routes.configured_concurrency_limits(),
            [
                ("vendor-default".to_owned(), 3),
                ("vendor-code".to_owned(), 7)
            ]
        );
    }

    #[test]
    fn launch_scoped_effort_includes_configured_acp_thinking_placeholder() {
        let mut cline = route("qwen/qwen3.8-max", BackendKind::ConfiguredAcp);
        cline.effort = Some("high".to_owned());
        cline.acp = Some(AcpLaunch {
            program: "cline".to_owned(),
            arguments: vec![
                "--thinking".to_owned(),
                "{effort}".to_owned(),
                "-m".to_owned(),
                "{model}".to_owned(),
                "--acp".to_owned(),
            ],
        });
        let mut cursor = route("auto", BackendKind::ConfiguredAcp);
        cursor.effort = Some("high".to_owned());
        cursor.acp = Some(AcpLaunch {
            program: "cursor-agent".to_owned(),
            arguments: vec![
                "--model".to_owned(),
                "{model}".to_owned(),
                "--yolo".to_owned(),
                "acp".to_owned(),
            ],
        });
        let routes = RoutedBackends::lazy(&[cline, cursor]);
        assert_eq!(
            routes.launch_scoped_effort("qwen/qwen3.8-max").as_deref(),
            Some("high")
        );
        assert_eq!(routes.launch_scoped_effort("auto"), None);
    }

    #[test]
    fn applies_codex_provider_and_catalog_to_exact_and_dynamic_routes() {
        let mut fugu = route("fugu", BackendKind::CodexAppServer);
        fugu.model_provider = Some("sakana".to_owned());
        fugu.model_catalog_json = Some("~/.codex/fugu.json".to_owned());
        fugu.model_prefixes.push("fugu".to_owned());
        let routes = RoutedBackends::lazy(&[fugu]);

        for model in ["fugu", "fugu-ultra-v1.1"] {
            let (_, route) = routes.resolve(model).expect("Fugu route");
            let params = route.thread_start_params(serde_json::json!({
                "model": model,
                "config": {"web_search":"disabled"}
            }));
            assert_eq!(params["modelProvider"], "sakana");
            assert!(
                params["config"]["model_catalog_json"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("/.codex/fugu.json"))
            );
            assert_eq!(params["config"]["web_search"], "disabled");
        }
        let mut plain = route("plain", BackendKind::CodexAppServer);
        plain.model_catalog_json = Some("catalog.json".to_owned());
        let (_, plain) = RoutedBackends::lazy(&[plain]).resolve("plain").unwrap();
        let params = plain.thread_start_params(serde_json::json!({}));
        assert_eq!(params["config"]["model_catalog_json"], "catalog.json");
    }

    #[test]
    fn selects_web_search_mode_from_the_most_specific_prefix() {
        let mut broad = route("broad", BackendKind::CodexAppServer);
        broad.web_search_mode = WebSearchMode::DelegateCcr;
        broad.model_prefixes.push("vendor-".to_owned());
        let mut specific = route("specific", BackendKind::CodexAppServer);
        specific.web_search_mode = WebSearchMode::CodexNative;
        specific.model_prefixes.push("vendor-codex-".to_owned());
        let routes = RoutedBackends::lazy(&[broad, specific]);

        assert_eq!(
            routes.web_search_mode("vendor-chat"),
            WebSearchMode::DelegateCcr
        );
        assert_eq!(
            routes.web_search_mode("vendor-codex-preview"),
            WebSearchMode::CodexNative
        );
        assert_eq!(routes.web_search_mode("unknown"), WebSearchMode::default());
    }

    #[test]
    fn exact_web_search_route_overrides_a_matching_prefix() {
        let mut template = route("template", BackendKind::CodexAppServer);
        template.web_search_mode = WebSearchMode::CodexNative;
        template.model_prefixes.push("vendor-".to_owned());
        let mut exact = route("vendor-model", BackendKind::CodexAppServer);
        exact.web_search_mode = WebSearchMode::Disabled;
        let routes = RoutedBackends::lazy(&[template, exact]);

        assert_eq!(
            routes.web_search_mode("vendor-model"),
            WebSearchMode::Disabled
        );
        assert_eq!(
            routes.web_search_mode("vendor-preview"),
            WebSearchMode::CodexNative
        );
    }

    #[test]
    fn selects_exact_dynamic_and_prefix_context_limits() {
        let mut exact = route("exact", BackendKind::CodexAppServer);
        exact.max_context_tokens = Some(100);
        let mut prefixed = route("prefix", BackendKind::CodexAppServer);
        prefixed.max_context_tokens = Some(200);
        prefixed.model_prefixes.push("prefix-".to_owned());
        let routes = RoutedBackends::lazy(&[exact, prefixed]);

        assert_eq!(routes.max_context_tokens_for_model("exact"), Some(100));
        assert_eq!(
            routes.max_context_tokens_for_model("prefix-chat"),
            Some(200)
        );
        routes.resolve("prefix-dynamic").expect("dynamic route");
        assert_eq!(
            routes.max_context_tokens_for_model("prefix-dynamic"),
            Some(200)
        );
        assert_eq!(routes.max_context_tokens_for_model("unknown"), None);
    }

    #[tokio::test]
    async fn get_does_not_return_a_dead_ready_backend() {
        let dead = Arc::new(AgentBackend::Grok(
            crate::grok_acp::GrokAcp::stopped_for_test(),
        ));
        assert!(!dead.is_alive());
        let route = super::RoutedBackend::ready("dead-grok".into(), dead);
        assert!(route.ready_backend().is_none());
        assert_get_avoids_dead_ready_backend(route).await;
    }

    #[tokio::test]
    async fn retired_startup_cannot_republish_a_stale_backend_to_waiting_get() {
        let mut template = route("stale-acp", BackendKind::ConfiguredAcp);
        template.acp = Some(AcpLaunch {
            program: "/definitely/missing/stale-acp".to_owned(),
            arguments: vec!["--stdio".to_owned()],
        });
        let startup = Arc::new(super::BackendStartup::default());
        let route = Arc::new(super::RoutedBackend::lazy(template, Arc::clone(&startup)));
        let (sender, receiver) = tokio::sync::watch::channel(StartupState::Starting);
        *startup.receiver.lock().expect("backend startup poisoned") = Some(receiver);

        let waiting = spawn_route_get(Arc::clone(&route));
        // Let `get` clone the Starting receiver before invalidating it. This
        // makes the stale-result race deterministic rather than timing based.
        tokio::task::yield_now().await;
        route.retire();

        let stale = Arc::new(AgentBackend::Grok(
            crate::grok_acp::GrokAcp::alive_for_test(),
        ));
        startup::publish_result(sender, Ok(Arc::clone(&stale))).await;

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .expect("retired startup waiter must settle")
            .expect("retired startup waiter task must not panic");
        assert!(result.is_err(), "stale backend must never be returned");
        assert!(
            !stale.is_alive(),
            "stale provider must be shut down after the route generation is retired"
        );
    }

    #[tokio::test]
    async fn pool_shutdown_fences_a_waiting_startup_from_a_late_spawn_result() {
        let mut template = route("shutdown-stale-acp", BackendKind::ConfiguredAcp);
        template.acp = Some(AcpLaunch {
            program: "/definitely/missing/shutdown-stale-acp".to_owned(),
            arguments: vec!["--stdio".to_owned()],
        });
        let routes = RoutedBackends::lazy(&[template]);
        let route = routes.route(0);
        let (sender, receiver) = tokio::sync::watch::channel(StartupState::Starting);
        *route
            .startup
            .receiver
            .lock()
            .expect("backend startup poisoned") = Some(receiver);

        let waiting = spawn_route_get(Arc::clone(&route));
        tokio::task::yield_now().await;
        routes.shutdown().await;

        let stale = Arc::new(AgentBackend::Grok(
            crate::grok_acp::GrokAcp::alive_for_test(),
        ));
        startup::publish_result(sender, Ok(Arc::clone(&stale))).await;

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .expect("shutdown waiter must settle")
            .expect("shutdown waiter task must not panic");
        assert!(result.is_err(), "shutdown must not return a stale backend");
        assert!(
            !stale.is_alive(),
            "late spawn must be cleaned up after shutdown"
        );
    }

    fn spawn_route_get(
        route: Arc<super::RoutedBackend>,
    ) -> tokio::task::JoinHandle<anyhow::Result<Arc<AgentBackend>>> {
        tokio::spawn(async move { route.get().await })
    }

    async fn assert_get_avoids_dead_ready_backend(route: super::RoutedBackend) {
        let result = tokio::time::timeout(std::time::Duration::from_millis(80), route.get()).await;
        let Ok(Ok(backend)) = result else {
            return;
        };
        assert!(
            backend.is_alive(),
            "get() must not return a dead Ready backend"
        );
    }

    #[tokio::test]
    async fn failed_startup_is_not_alive() {
        let route = BackendRoute {
            model: "missing-acp".to_owned(),
            backend: BackendKind::ConfiguredAcp,
            effort: None,
            model_provider: None,
            model_catalog_json: None,
            pi_provider: None,
            pi_model: None,
            max_context_tokens: None,
            model_prefixes: Vec::new(),
            max_concurrency: None,
            acp: Some(AcpLaunch {
                program: "/definitely/missing/acp".to_owned(),
                arguments: vec!["--stdio".to_owned()],
            }),
            web_search_mode: WebSearchMode::default(),
        };
        let routes = RoutedBackends::lazy(&[route]);
        assert!(routes.model_is_alive("missing-acp"));
        assert!(routes.route(0).get().await.is_err());
        assert!(!routes.model_is_alive("missing-acp"));
        assert!(routes.route(0).get().await.is_err());
        assert!(routes.model_is_alive("unconfigured-model"));
        assert!(routes.is_alive());
    }

    #[tokio::test]
    async fn closed_startup_receiver_reaps_a_successful_backend() {
        let (sender, receiver) = tokio::sync::watch::channel(StartupState::Starting);
        drop(receiver);
        let backend = Arc::new(AgentBackend::Routed(RoutedBackends::lazy(&[])));

        startup::publish_result(sender, Ok(backend)).await;
    }

    #[tokio::test]
    async fn closed_startup_receiver_discards_a_failed_backend_result() {
        let (sender, receiver) = tokio::sync::watch::channel(StartupState::Starting);
        drop(receiver);

        startup::publish_result(sender, Err(Arc::<str>::from("startup failed"))).await;
    }

    #[tokio::test]
    async fn open_startup_receiver_gets_the_started_backend() {
        let (sender, receiver) = tokio::sync::watch::channel(StartupState::Starting);
        let backend = Arc::new(AgentBackend::Routed(RoutedBackends::lazy(&[])));

        startup::publish_result(sender, Ok(backend)).await;

        assert!(matches!(
            receiver.borrow().clone(),
            StartupState::Ready(Ok(_))
        ));
    }

    #[tokio::test]
    async fn shutdown_skips_routes_without_ready_backends() {
        let routes = RoutedBackends::lazy(&[
            route("no-startup", BackendKind::GrokAcp),
            route("starting", BackendKind::GrokAcp),
            route("failed", BackendKind::GrokAcp),
        ]);
        let starting = routes.route(1);
        let (_, starting_receiver) = tokio::sync::watch::channel(StartupState::Starting);
        *starting
            .startup
            .receiver
            .lock()
            .expect("backend startup poisoned") = Some(starting_receiver);
        let failed = routes.route(2);
        let (_, failed_receiver) = tokio::sync::watch::channel(StartupState::Ready(Err(
            Arc::<str>::from("startup failed"),
        )));
        *failed
            .startup
            .receiver
            .lock()
            .expect("backend startup poisoned") = Some(failed_receiver);

        routes.shutdown().await;
    }

    fn route(model: &str, backend: BackendKind) -> BackendRoute {
        BackendRoute::new(model, backend)
    }

    fn route_with_prefix(model: &str, backend: BackendKind, prefix: &str) -> BackendRoute {
        let mut route = BackendRoute::new(model, backend);
        route.model_prefixes.push(prefix.to_owned());
        route
    }

    /// A route whose startup channel the test drives by hand instead of
    /// spawning a real provider process.
    fn manual_channel_route(
        model: &str,
    ) -> (
        Arc<super::RoutedBackend>,
        Arc<super::BackendStartup>,
        tokio::sync::watch::Sender<StartupState>,
    ) {
        let startup = Arc::new(super::BackendStartup::default());
        let (sender, receiver) = tokio::sync::watch::channel(StartupState::Starting);
        *startup.receiver.lock().expect("backend startup poisoned") = Some(receiver);
        let route = Arc::new(super::RoutedBackend::lazy(
            route(model, BackendKind::GrokAcp),
            Arc::clone(&startup),
        ));
        (route, startup, sender)
    }

    /// Runs `route.get()` on a background task so the caller can drive the
    /// startup channel (publish a result, drop the sender, ...) while it
    /// waits.
    fn spawn_get(
        route: &Arc<super::RoutedBackend>,
    ) -> tokio::task::JoinHandle<anyhow::Result<Arc<AgentBackend>>> {
        let route = Arc::clone(route);
        tokio::spawn(async move { route.get().await })
    }

    #[tokio::test]
    async fn closed_pool_rejects_get_ready_backend_and_is_alive() {
        let routes = RoutedBackends::lazy(&[route("closed-model", BackendKind::GrokAcp)]);
        routes.shutdown().await;
        let closed = routes.route(0);
        assert!(closed.ready_backend().is_none());
        assert!(!closed.is_alive());
        assert!(closed.get().await.is_err());
    }

    #[tokio::test]
    async fn retired_route_subscription_is_closed_instead_of_panicking() {
        let routes = RoutedBackends::lazy(&[route("retired-model", BackendKind::GrokAcp)]);
        routes.route(0).retire();
        let backend = AgentBackend::Routed(routes);
        let events = backend.subscribe_thread("0:target");

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), events.recv())
                .await
                .expect("retired route subscription must settle")
                .is_none()
        );
    }

    #[tokio::test]
    async fn get_returns_the_alive_backend_once_startup_publishes_ready() {
        let (route, _startup, sender) = manual_channel_route("alive-model");
        let waiting = spawn_get(&route);
        tokio::task::yield_now().await;
        let alive = Arc::new(AgentBackend::Grok(
            crate::grok_acp::GrokAcp::alive_for_test(),
        ));
        startup::publish_result(sender, Ok(Arc::clone(&alive))).await;

        let backend = tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .expect("get must settle")
            .expect("get task must not panic")
            .expect("alive backend must be returned");
        assert!(Arc::ptr_eq(&backend, &alive));
    }

    #[tokio::test]
    async fn get_reports_an_error_when_startup_stops_without_a_result() {
        let (route, _startup, sender) = manual_channel_route("stopped-model");
        let waiting = spawn_get(&route);
        tokio::task::yield_now().await;
        drop(sender);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
                .await
                .expect("get must settle")
                .expect("get task must not panic")
                .is_err()
        );
    }

    #[tokio::test]
    async fn retry_stale_backend_detects_a_generation_that_never_advanced() {
        let (route, startup, sender) = manual_channel_route("stable-model");
        let alive = Arc::new(AgentBackend::Grok(
            crate::grok_acp::GrokAcp::alive_for_test(),
        ));
        // Publish once so the still-alive backend becomes the reusable state
        // that `startup_receiver` will hand back without bumping generation.
        startup::publish_result(sender, Ok(Arc::clone(&alive))).await;
        let current_generation = startup
            .generation
            .load(std::sync::atomic::Ordering::Acquire);
        let stale = Arc::new(AgentBackend::Grok(
            crate::grok_acp::GrokAcp::alive_for_test(),
        ));

        let result = route.retry_stale_backend(current_generation, stale).await;
        assert!(
            result.is_err(),
            "a generation that never advances past a reusable startup must be reported"
        );
    }

    /// Signals `ready` as soon as the reader thread starts, then blocks on
    /// `startup_receiver`'s internal lock until the caller releases it.
    fn spawn_startup_receiver_reader(
        route: &Arc<super::RoutedBackend>,
        ready: std::sync::mpsc::Sender<()>,
    ) -> std::thread::JoinHandle<anyhow::Result<(u64, tokio::sync::watch::Receiver<StartupState>)>>
    {
        let route = Arc::clone(route);
        std::thread::spawn(move || {
            ready.send(()).expect("signal reader started");
            route.startup_receiver()
        })
    }

    #[test]
    fn startup_receiver_rejects_a_pool_closed_while_it_waited_for_the_lock() {
        let route = Arc::new(super::RoutedBackend::lazy(
            route("late-close", BackendKind::GrokAcp),
            Arc::new(super::BackendStartup::default()),
        ));
        let guard = route
            .startup
            .receiver
            .lock()
            .expect("backend startup poisoned");
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let reader = spawn_startup_receiver_reader(&route, ready_tx);
        ready_rx.recv().expect("reader signal");
        // Give the reader time to pass the pre-lock `closed` check and block
        // on the mutex that `guard` still holds.
        std::thread::sleep(std::time::Duration::from_millis(20));
        route
            .startup
            .closed
            .store(true, std::sync::atomic::Ordering::Release);
        drop(guard);
        assert!(
            reader
                .join()
                .expect("startup_receiver reader thread")
                .is_err(),
            "a pool closed while a caller waited on the lock must still be rejected"
        );
    }
}
