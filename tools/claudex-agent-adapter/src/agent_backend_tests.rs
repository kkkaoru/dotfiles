#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{AcpLaunch, AgentBackend, BackendKind, BackendRoute};

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
            max_context_tokens: None,
            model_prefixes: Vec::new(),
            acp: Some(AcpLaunch {
                program: "provider".to_owned(),
                arguments: vec!["--stdio".to_owned()],
            }),
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

    fn route_with_prefix(model: &str, backend: BackendKind, prefix: &str) -> BackendRoute {
        let mut route = BackendRoute::new(model, backend);
        route.model_prefixes.push(prefix.to_owned());
        route
    }

    #[test]
    #[should_panic(expected = "a routed backend has no single kind")]
    fn routed_backend_rejects_a_leaf_kind_query() {
        AgentBackend::spawn_routes(&[BackendRoute::new(
            "model",
            BackendKind::CodexAppServer,
        )])
        .kind();
    }
}
