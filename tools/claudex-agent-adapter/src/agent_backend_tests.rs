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
            model_prefixes: Vec::new(),
            acp: Some(AcpLaunch {
                program: "provider".to_owned(),
                arguments: vec!["--stdio".to_owned()],
            }),
        };
        assert!(configured.description().contains("configured-acp"));
        let routes = AgentBackend::spawn_routes(&[
            BackendRoute::new("unused-codex", BackendKind::CodexAppServer),
            BackendRoute::new("unused-copilot", BackendKind::CopilotAcp),
            BackendRoute::new("unused-grok", BackendKind::GrokAcp),
        ]);
        assert!(routes.started_models().is_empty());
        assert!(routes.is_alive());
        for model in ["gpt", "gpt-5.6-sol", "gpt_custom", "grok", "grok-4.5"] {
            assert!(routes.supports_model(model));
        }
        for model in ["", "GPT-5.6-sol", "Grok-4.5", "claude-unconfigured"] {
            assert!(!routes.supports_model(model));
        }
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
