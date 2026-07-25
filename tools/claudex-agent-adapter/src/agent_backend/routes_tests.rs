#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;

    use super::{MAX_DYNAMIC_ROUTES, RoutedBackends};
    use crate::agent_backend::{AcpLaunch, BackendKind, BackendRoute};

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

    #[tokio::test]
    async fn failed_startup_is_not_alive() {
        let route = BackendRoute {
            model: "missing-acp".to_owned(),
            backend: BackendKind::ConfiguredAcp,
            max_context_tokens: None,
            model_prefixes: Vec::new(),
            acp: Some(AcpLaunch {
                program: "/definitely/missing/acp".to_owned(),
                arguments: vec!["--stdio".to_owned()],
            }),
        };
        let routes = RoutedBackends::lazy(&[route]);
        assert!(routes.model_is_alive("missing-acp"));
        assert!(routes.route(0).get().await.is_err());
        assert!(!routes.model_is_alive("missing-acp"));
        assert!(routes.model_is_alive("unconfigured-model"));
        assert!(routes.is_alive());
    }

    fn route(model: &str, backend: BackendKind) -> BackendRoute {
        BackendRoute::new(model, backend)
    }

    fn route_with_prefix(model: &str, backend: BackendKind, prefix: &str) -> BackendRoute {
        let mut route = BackendRoute::new(model, backend);
        route.model_prefixes.push(prefix.to_owned());
        route
    }
}
