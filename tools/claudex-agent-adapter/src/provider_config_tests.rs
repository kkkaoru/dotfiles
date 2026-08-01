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
            r#"{"id":"p","agent":"w","defaultModel":"m","effort":"high","modelPrefixes":["m-"],"backend":"grok-acp"}"#,
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
        assert_eq!(loaded.routes[0].model, "model");
        assert_eq!(loaded.routes[0].effort.as_deref(), Some("high"));
        assert_eq!(loaded.routes.len(), 1);
        assert_eq!(loaded.routes[0].model_prefixes, ["model-"]);
        // Disabled providers still contribute catalog identities for remaps.
        assert!(loaded.model_catalog.matches("model"));
        assert!(loaded.model_catalog.matches("model-spark"));
        assert!(loaded.model_catalog.matches("model-extra"));
        assert!(loaded.model_catalog.matches("off"));
        assert!(!loaded.model_catalog.matches("claude-sonnet-5"));
        assert_eq!(
            loaded.model_catalog.worker_fields("worker"),
            Some(("model-spark", "high"))
        );
        assert_eq!(loaded.model_catalog.worker_fields("off"), None);
    }

    #[test]
    fn loads_native_workers_without_declaring_a_provider_backend() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("providers.json");
        let mut document: serde_json::Value = serde_json::from_str(&config(
            r#"{"id":"p","agent":"worker","defaultModel":"model","effort":"high","enabled":true,"backend":"grok-acp"}"#,
        ))
        .unwrap();
        document["nativeWorkers"] = serde_json::json!([
            {
                "agent": "claudex-haiku-search",
                "model": "claude-haiku-4-5",
                "effort": "max"
            }
        ]);
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();

        let loaded = load(&path).expect("load native worker");
        assert_eq!(
            loaded.model_catalog.worker_fields("claudex-haiku-search"),
            Some(("claude-haiku-4-5", "max"))
        );
        assert!(loaded
            .model_catalog
            .worker_routes()
            .iter()
            .any(|worker| worker.agent == "claudex-haiku-search"));
        // Native workers must stay out of the provider identity catalog so an
        // explicit Haiku child reaches the Claude subscription backend.
        assert!(!loaded.model_catalog.matches("claude-haiku-4-5"));
    }

    #[test]
    fn validates_and_exports_worker_routes() {
        let mut catalog = ModelCatalog::from_routes(&[BackendRoute::new("model", BackendKind::GrokAcp)]);
        assert!(catalog
            .set_worker_routes(vec![WorkerRoute {
                agent: String::new(),
                model: "model".to_owned(),
                effort: "high".to_owned(),
            }])
            .is_err());
        assert!(catalog
            .set_worker_routes(vec![
                WorkerRoute {
                    agent: "worker".to_owned(),
                    model: "model".to_owned(),
                    effort: "high".to_owned(),
                },
                WorkerRoute {
                    agent: "worker".to_owned(),
                    model: "other".to_owned(),
                    effort: "low".to_owned(),
                },
            ])
            .is_err());
        catalog
            .set_worker_routes(vec![WorkerRoute {
                agent: "worker".to_owned(),
                model: "model".to_owned(),
                effort: "high".to_owned(),
            }])
            .expect("valid worker route");
        assert_eq!(catalog.worker_fields("worker"), Some(("model", "high")));
        assert_eq!(catalog.worker_effort_for_model("model"), Some("high"));
        assert_eq!(catalog.worker_routes().len(), 1);
    }

    #[test]
    fn loads_configured_web_search_fallback_workers() {
        let root = tempfile::tempdir().expect("web search config fixture");
        let path = root.path().join("providers.json");
        let mut document: serde_json::Value = serde_json::from_str(&config(
            r#"{"id":"p","agent":"worker","defaultModel":"model","subagentModel":"worker-model","effort":"high","enabled":true,"backend":"codex-app-server"}"#,
        ))
        .unwrap();
        document["webSearch"] = serde_json::json!({"fallbackProviders":["p"]});
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();

        let loaded = load(&path).expect("load search fallback");
        assert_eq!(
            loaded.model_catalog.search_worker_routes(),
            [WorkerRoute {
                agent: "worker".to_owned(),
                model: "worker-model".to_owned(),
                effort: "high".to_owned()
            }]
        );

        document["webSearch"] = serde_json::json!({"fallbackProviders":["missing"]});
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        let error = load(&path).err().expect("missing search fallback should fail");
        assert!(error.to_string().contains("not enabled"));
    }

    #[test]
    fn accepts_and_validates_the_optional_advisor_choice() {
        let mut document: serde_json::Value = serde_json::from_str(&config(
            r#"{"id":"p","agent":"worker","defaultModel":"model","effort":"high","enabled":true,"backend":"grok-acp"}"#,
        ))
        .unwrap();
        document["advisor"] = serde_json::json!({
            "agent": "custom-advisor",
            "model": "claude-fable-5",
            "effort": "xhigh"
        });
        let parsed: ProviderConfig = serde_json::from_value(document.clone()).unwrap();
        assert!(validate(parsed).is_ok());

        document["advisor"]["model"] = serde_json::Value::String(String::new());
        let parsed: ProviderConfig = serde_json::from_value(document).unwrap();
        assert!(validate(parsed).is_err());
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
    fn accepts_open_code_go_request_budget_metadata() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("providers.json");
        std::fs::write(
            &path,
            config(
                r#"{"id":"p","agent":"worker","defaultModel":"opencode-go/deepseek-v4-flash","effort":"high","enabled":true,"usageProvider":"opencodego","requestBudget":{"estimatedRequests":31650,"windowMinutes":300,"usageWindow":"primary"},"modelPrefixes":["opencode-go/"],"backend":"configured-acp","acp":{"program":"opencode","arguments":["acp"]}}"#,
            ),
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.routes[0].model, "opencode-go/deepseek-v4-flash");
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
                r#"{"id":"p","agent":"claudex-ollama-glm-5-2","defaultModel":"glm-5.2:cloud","subagentModel":"glm-5.2:cloud","effort":"max","enabled":true,"usageProvider":"ollama","modelProvider":"ollama","modelCatalogJson":"~/.codex/fugu.json","modelPrefixes":["glm-"],"backend":"codex-app-server"}"#,
            ),
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.routes[0].model, "glm-5.2:cloud");
        assert_eq!(
            loaded.routes[0].model_provider.as_deref(),
            Some("ollama")
        );
        assert_eq!(
            loaded.routes[0].model_catalog_json.as_deref(),
            Some("~/.codex/fugu.json")
        );
        assert!(loaded.model_catalog.matches("glm-5.2:cloud"));
        assert_eq!(
            loaded
                .model_catalog
                .worker_fields("claudex-ollama-glm-5-2"),
            Some(("glm-5.2:cloud", "max"))
        );
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
    fn accepts_only_native_grok_reasoning_efforts() {
        for effort in ["low", "medium", "high"] {
            let parsed: ProviderConfig = serde_json::from_str(&config(&format!(
                r#"{{"id":"p","agent":"w","defaultModel":"grok-4.5","effort":"{effort}","enabled":true,"backend":"grok-acp"}}"#
            )))
            .unwrap();
            assert!(validate(parsed).is_ok(), "rejected Grok effort {effort}");
        }
        for effort in ["mid", "xhigh", "max"] {
            let parsed: ProviderConfig = serde_json::from_str(&config(&format!(
                r#"{{"id":"p","agent":"w","defaultModel":"grok-4.5","effort":"{effort}","enabled":true,"backend":"grok-acp"}}"#
            )))
            .unwrap();
            assert!(validate(parsed).is_err(), "accepted Grok effort {effort}");
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

        let empty = ModelCatalog::from_routes(&[BackendRoute::new("", BackendKind::GrokAcp)]);
        assert!(!empty.matches(""));
    }
}
