#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;

    use super::{MAX_DYNAMIC_ROUTES, RoutedBackends};
    use crate::agent_backend::{AcpLaunch, BackendKind, BackendRoute};

    #[test]
    fn shares_codex_startup_but_keeps_acp_servers_model_specific() {
        let routes = RoutedBackends::lazy(&[
            route("gpt-one", BackendKind::CodexAppServer),
            route("gpt-two", BackendKind::CodexAppServer),
            route("gpt-copilot-one", BackendKind::CopilotAcp),
            route("gpt-copilot-two", BackendKind::CopilotAcp),
            route("grok-one", BackendKind::GrokAcp),
            route("grok-two", BackendKind::GrokAcp),
        ]);
        let gpt_one = routes.route(0);
        let gpt_two = routes.route(1);
        let copilot_one = routes.route(2);
        let copilot_two = routes.route(3);
        let grok_one = routes.route(4);
        let grok_two = routes.route(5);

        assert!(Arc::ptr_eq(&gpt_one.startup, &gpt_two.startup));
        assert!(!Arc::ptr_eq(&copilot_one.startup, &copilot_two.startup));
        assert!(!Arc::ptr_eq(&grok_one.startup, &grok_two.startup));

        let (_, dynamic_gpt) = routes.resolve("gpt-dynamic").unwrap();
        let (_, dynamic_grok) = routes.resolve("grok-dynamic").unwrap();
        assert!(Arc::ptr_eq(&gpt_one.startup, &dynamic_gpt.startup));
        assert!(!Arc::ptr_eq(&grok_one.startup, &dynamic_grok.startup));
    }

    #[test]
    fn bounds_dynamic_routes_but_reuses_existing_models() {
        let routes = RoutedBackends::lazy(&[]);
        for index in 0..MAX_DYNAMIC_ROUTES {
            let (route_index, route) = routes
                .resolve(&format!("gpt-dynamic-{index}"))
                .expect("available dynamic route");
            assert_eq!(route_index, index);
            assert_eq!(route.model, format!("gpt-dynamic-{index}"));
        }
        let (existing, _) = routes.resolve("gpt-dynamic-0").expect("existing route");
        assert_eq!(existing, 0);
        assert_eq!(routes.route(existing).model, "gpt-dynamic-0");
        assert_eq!(routes.find("gpt-dynamic-0").unwrap().model, "gpt-dynamic-0");
        assert!(routes.find("missing").is_none());
        assert!(routes.first_ready(BackendKind::CodexAppServer).is_none());
        assert!(routes.resolve("grok-over-limit").is_err());
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
    }

    #[tokio::test]
    async fn failed_startup_is_not_alive() {
        let route = BackendRoute {
            model: "missing-acp".to_owned(),
            backend: BackendKind::ConfiguredAcp,
            model_prefixes: Vec::new(),
            acp: Some(AcpLaunch {
                program: "/definitely/missing/acp".to_owned(),
                arguments: vec!["--stdio".to_owned()],
            }),
        };
        let routes = RoutedBackends::lazy(&[route]);
        assert!(routes.route(0).get().await.is_err());
        assert!(!routes.is_alive());
    }

    fn route(model: &str, backend: BackendKind) -> BackendRoute {
        BackendRoute::new(model, backend)
    }
}
