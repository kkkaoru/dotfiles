#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::agent_backend::{BackendKind, BackendRoute};

    use super::*;

    fn config(provider: &str) -> String {
        format!(
            r#"{{"version":1,"mainProviders":["p"],"providers":[{provider}],"fallback":{{"agent":"f","model":"m","effort":"high"}}}}"#
        )
    }

    fn parsed() -> ProviderConfig {
        serde_json::from_str(&config(
            r#"{"id":"p","agent":"w","defaultModel":"m","effort":"h","modelPrefixes":["m-"],"backend":"grok-acp"}"#,
        ))
        .unwrap()
    }

    #[test]
    fn loads_enabled_routes_and_ignores_disabled_routes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("providers.json");
        std::fs::write(
            &path,
            config(
                r#"{"id":"p","agent":"worker","defaultModel":"model","subagentModel":"model-spark","effort":"high","enabled":true,"usageProvider":"quota","modelPrefixes":["model-"],"backend":"codex-app-server"},{"id":"off","agent":"off","defaultModel":"off","effort":"low","enabled":false,"backend":"grok-acp"}"#,
            ),
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.main_model, "model");
        assert_eq!(loaded.routes[0].model, "model");
        assert_eq!(loaded.routes.len(), 1);
        assert_eq!(loaded.routes[0].model_prefixes, ["model-"]);
        // Disabled providers still contribute catalog identities for remaps.
        assert!(loaded.model_catalog.matches("model"));
        assert!(loaded.model_catalog.matches("model-spark"));
        assert!(loaded.model_catalog.matches("model-extra"));
        assert!(loaded.model_catalog.matches("off"));
        assert!(!loaded.model_catalog.matches("claude-sonnet-5"));
    }

    #[test]
    fn accepts_a_configured_acp() {
        let json = config(
            r#"{"id":"p","agent":"worker","defaultModel":"new-1","effort":"high","enabled":true,"modelPrefixes":["new-"],"backend":"configured-acp","acp":{"program":"new-acp","arguments":["--model","{model}","--stdio"]}}"#,
        );
        let parsed: ProviderConfig = serde_json::from_str(&json).unwrap();
        let loaded = validate(parsed).unwrap();
        assert_eq!(loaded.routes[0].acp.as_ref().unwrap().program, "new-acp");
        assert_eq!(loaded.routes[0].max_context_tokens, None);
    }

    #[test]
    fn accepts_provider_context_limit() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("providers.json");
        std::fs::write(
            &path,
            config(
                r#"{"id":"p","agent":"worker","defaultModel":"new-1","effort":"high","enabled":true,"maxContextTokens":262144,"modelPrefixes":["new-"],"backend":"codex-app-server"}"#,
            ),
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.routes[0].max_context_tokens, Some(262_144));
    }

    #[test]
    fn accepts_provider_concurrency_limit() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("providers.json");
        std::fs::write(
            &path,
            config(
                r#"{"id":"p","agent":"worker","defaultModel":"new-1","effort":"high","enabled":true,"maxConcurrency":7,"modelPrefixes":["new-"],"backend":"codex-app-server"}"#,
            ),
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.routes[0].max_concurrency, Some(7));
        assert!(loaded.routes[0].description().contains("maxConcurrency"));
    }

    #[test]
    fn accepts_codex_provider_and_catalog_metadata() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("providers.json");
        std::fs::write(
            &path,
            config(
                r#"{"id":"p","agent":"worker","defaultModel":"fugu","effort":"high","enabled":true,"modelProvider":"sakana","modelCatalogJson":"~/.codex/fugu.json","modelPrefixes":["fugu"],"backend":"codex-app-server"}"#,
            ),
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.routes[0].model_provider.as_deref(), Some("sakana"));
        assert_eq!(
            loaded.routes[0].model_catalog_json.as_deref(),
            Some("~/.codex/fugu.json")
        );
    }

    #[test]
    fn accepts_ollama_cloud_codex_provider_metadata() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("providers.json");
        std::fs::write(
            &path,
            config(
                r#"{"id":"p","agent":"claudex-ollama-glm-5-2","defaultModel":"glm-5.2:cloud","subagentModel":"glm-5.2:cloud","effort":"high","enabled":true,"usageProvider":"ollama","modelProvider":"ollama-launch-codex-app","modelCatalogJson":"~/.codex/fugu.json","modelPrefixes":["glm-"],"backend":"codex-app-server"}"#,
            ),
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.main_model, "glm-5.2:cloud");
        assert_eq!(
            loaded.routes[0].model_provider.as_deref(),
            Some("ollama-launch-codex-app")
        );
        assert_eq!(
            loaded.routes[0].model_catalog_json.as_deref(),
            Some("~/.codex/fugu.json")
        );
        assert!(loaded.model_catalog.matches("glm-5.2:cloud"));
    }

    #[test]
    fn rejects_invalid_configurations() {
        let invalid = [
            config(
                r#"{"id":"p","agent":"w","defaultModel":"m","effort":"h","enabled":true,"backend":"configured-acp"}"#,
            ),
            config(
                r#"{"id":"p","agent":"w","defaultModel":"m","effort":"h","enabled":true,"backend":"grok-acp","acp":{"program":"x","arguments":["y"]}}"#,
            ),
            config(
                r#"{"id":"p","agent":"","defaultModel":"m","effort":"h","enabled":true,"backend":"grok-acp"}"#,
            ),
            config(
                r#"{"id":"p","agent":"w","defaultModel":"m","effort":"h","backend":"configured-acp","acp":{"program":"","arguments":["--stdio"]}}"#,
            ),
            config(
                r#"{"id":"p","agent":"w","defaultModel":"m","effort":"h","backend":"configured-acp","acp":{"program":"provider","arguments":[]}}"#,
            ),
            config(
                r#"{"id":"p","agent":"w","defaultModel":"m","effort":"h","enabled":true,"maxContextTokens":0,"backend":"grok-acp"}"#,
            ),
            config(
                r#"{"id":"p","agent":"w","defaultModel":"m","effort":"h","enabled":true,"maxConcurrency":0,"backend":"grok-acp"}"#,
            ),
            config(
                r#"{"id":"p","agent":"w","defaultModel":"m","subagentModel":"","effort":"h","enabled":true,"backend":"grok-acp"}"#,
            ),
            config(
                r#"{"id":"p","agent":"w","defaultModel":"m","effort":"h","enabled":true,"modelProvider":"","backend":"codex-app-server"}"#,
            ),
            config(
                r#"{"id":"p","agent":"w","defaultModel":"m","effort":"h","enabled":true,"modelCatalogJson":"","backend":"codex-app-server"}"#,
            ),
            config(
                r#"{"id":"p","agent":"w","defaultModel":"m","effort":"h","enabled":true,"modelProvider":"sakana","backend":"grok-acp"}"#,
            ),
        ];
        for json in invalid {
            let parsed: ProviderConfig = serde_json::from_str(&json).unwrap();
            assert!(validate(parsed).is_err());
        }
    }

    #[test]
    fn rejects_every_cross_provider_constraint() {
        let mut invalid = Vec::new();
        let mut config = parsed();
        config.version = 2;
        invalid.push(config);
        let mut config = parsed();
        config.providers[0].enabled = false;
        invalid.push(config);
        let mut config = parsed();
        config.main_providers = vec!["missing".to_owned()];
        invalid.push(config);
        let mut config = parsed();
        config.main_providers.clear();
        invalid.push(config);
        let mut config = parsed();
        config.main_providers = vec!["p".to_owned(), "p".to_owned()];
        invalid.push(config);
        let mut config = parsed();
        config.fallback.agent.clear();
        invalid.push(config);
        let mut config = parsed();
        config.providers[0].model_prefixes = vec![String::new()];
        invalid.push(config);
        let mut config = parsed();
        config.providers[0].max_concurrency = Some(crate::grok_acp::MAX_MODEL_CONCURRENCY + 1);
        invalid.push(config);
        for field in ["id", "model", "prefix"] {
            let mut config = parsed();
            let mut duplicate = config.providers[0].clone();
            match field {
                "id" => duplicate.default_model = "other".to_owned(),
                "model" => duplicate.id = "other".to_owned(),
                "prefix" => {
                    duplicate.id = "other".to_owned();
                    duplicate.default_model = "other".to_owned();
                }
                _ => unreachable!(),
            }
            config.providers.push(duplicate);
            invalid.push(config);
        }
        for config in invalid {
            assert!(validate(config).is_err());
        }
    }

    #[test]
    fn keeps_empty_disabled_catalog_entries_out_of_exact_matches() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("providers.json");
        std::fs::write(
            &path,
            config(
                r#"{"id":"p","agent":"worker","defaultModel":"model","effort":"high","enabled":true,"backend":"grok-acp"},{"id":"disabled","agent":"worker","defaultModel":"","effort":"low","enabled":false,"backend":"grok-acp"}"#,
            ),
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert!(!loaded.model_catalog.matches(""));

        let empty = ModelCatalog::from_routes(&[BackendRoute::new(
            "",
            BackendKind::GrokAcp,
        )]);
        assert!(!empty.matches(""));
    }
}
