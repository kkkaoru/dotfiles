#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{AcpLaunch, AgentBackend, BackendKind, BackendRoute, WebSearchMode};

    #[test]
    fn parses_and_displays_backend_kinds() {
        for (input, expected) in [
            ("codex-app-server", BackendKind::CodexAppServer),
            ("configured-acp", BackendKind::ConfiguredAcp),
            ("copilot-acp", BackendKind::CopilotAcp),
            ("grok-acp", BackendKind::GrokAcp),
        ] {
            assert_eq!(input.parse::<BackendKind>().unwrap(), expected);
            assert_eq!(expected.to_string(), input);
        }
        assert!("unknown".parse::<BackendKind>().is_err());
        assert!("=grok-acp".parse::<BackendRoute>().is_err());
        assert_eq!(
            "model=grok-acp".parse::<BackendRoute>().unwrap(),
            BackendRoute::new("model", BackendKind::GrokAcp)
        );
        assert!("invalid".parse::<BackendRoute>().is_err());
        let configured = BackendRoute {
            model: "configured".to_owned(),
            backend: BackendKind::ConfiguredAcp,
            model_provider: None,
            model_catalog_json: None,
            max_context_tokens: None,
            model_prefixes: Vec::new(),
            max_concurrency: None,
            acp: Some(AcpLaunch {
                program: "provider".to_owned(),
                arguments: vec!["--stdio".to_owned()],
            }),
            web_search_mode: WebSearchMode::default(),
        };
        assert!(configured.description().contains("configured-acp"));
        let routes = AgentBackend::spawn_routes(&[
            route_with_prefix("unused-codex", BackendKind::CodexAppServer, "codex-"),
            route_with_prefix("unused-copilot", BackendKind::CopilotAcp, "copilot-"),
            route_with_prefix("unused-acp", BackendKind::GrokAcp, "acp-"),
        ]);
        assert!(routes.started_models().is_empty());
        assert!(routes.is_alive());
        // Supports only exact configured models and declared prefixes — never vendor-name inference.
        for model in [
            "unused-codex",
            "codex-extra",
            "unused-copilot",
            "copilot-extra",
            "unused-acp",
            "acp-extra",
        ] {
            assert!(routes.supports_model(model));
        }
        for model in ["", "Codex-extra", "unconfigured", "gpt", "grok", "qwen"] {
            assert!(!routes.supports_model(model));
        }
    }

    #[test]
    fn keeps_plain_route_descriptions_compact() {
        assert_eq!(
            BackendRoute::new("model", BackendKind::CodexAppServer).description(),
            "model=codex-app-server"
        );
        let mut metadata = BackendRoute::new("model", BackendKind::CodexAppServer);
        metadata.model_provider = Some("provider".to_owned());
        assert!(metadata.description().contains("modelProvider"));

        let mut catalog = BackendRoute::new("model", BackendKind::CodexAppServer);
        catalog.model_catalog_json = Some("catalog.json".to_owned());
        assert!(catalog.description().contains("modelCatalogJson"));
        let mut context = BackendRoute::new("model", BackendKind::CodexAppServer);
        context.max_context_tokens = Some(100);
        assert!(context.description().contains("maxContextTokens"));
        let mut concurrency = BackendRoute::new("model", BackendKind::CodexAppServer);
        concurrency.max_concurrency = Some(2);
        assert!(concurrency.description().contains("maxConcurrency"));
        let mut prefix = BackendRoute::new("model", BackendKind::CodexAppServer);
        prefix.model_prefixes.push("model-".to_owned());
        assert!(prefix.description().contains("modelPrefixes"));
        let mut acp = BackendRoute::new("model", BackendKind::ConfiguredAcp);
        acp.acp = Some(AcpLaunch {
            program: "provider".to_owned(),
            arguments: Vec::new(),
        });
        assert!(acp.description().contains("program"));
    }

    #[test]
    fn describes_nondefault_web_search_routes_as_json() {
        for mode in [
            WebSearchMode::CodexNative,
            WebSearchMode::AcpNative,
            WebSearchMode::DelegateMcp,
            WebSearchMode::Disabled,
        ] {
            let mut route = BackendRoute::new("model", BackendKind::CodexAppServer);
            route.web_search_mode = mode;

            assert!(route.description().contains(mode.as_str()));
        }
    }

    fn route_with_prefix(model: &str, backend: BackendKind, prefix: &str) -> BackendRoute {
        let mut route = BackendRoute::new(model, backend);
        route.model_prefixes.push(prefix.to_owned());
        route
    }

    #[test]
    #[should_panic(expected = "a routed backend has no single kind")]
    fn routed_backend_rejects_a_leaf_kind_query() {
        AgentBackend::spawn_routes(&[BackendRoute::new("model", BackendKind::CodexAppServer)])
            .kind();
    }
}
