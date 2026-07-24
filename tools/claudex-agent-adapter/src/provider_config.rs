use std::{collections::HashSet, fs, path::Path};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::agent_backend::{AcpLaunch, BackendKind, BackendRoute};

const CONFIG_VERSION: u64 = 1;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ProviderConfig {
    version: u64,
    main_provider: String,
    providers: Vec<Provider>,
    fallback: AgentChoice,
    advisor: AgentChoice,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Provider {
    id: String,
    agent: String,
    default_model: String,
    effort: String,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    #[serde(default)]
    usage_provider: Option<String>,
    #[serde(default)]
    model_prefixes: Vec<String>,
    backend: BackendKind,
    #[serde(default)]
    acp: Option<AcpLaunch>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentChoice {
    agent: String,
    model: String,
    effort: String,
}

pub struct LoadedConfig {
    pub main_model: String,
    pub routes: Vec<BackendRoute>,
}

const fn enabled_by_default() -> bool {
    true
}

pub fn load(path: &Path) -> Result<LoadedConfig> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("read provider config {}", path.display()))?;
    let config: ProviderConfig = serde_json::from_str(&contents)
        .with_context(|| format!("parse provider config {}", path.display()))?;
    validate(config)
}

fn validate(config: ProviderConfig) -> Result<LoadedConfig> {
    if config.version != CONFIG_VERSION {
        bail!("provider config version must be {CONFIG_VERSION}");
    }
    validate_choice(&config.fallback, "fallback")?;
    validate_choice(&config.advisor, "advisor")?;
    let providers = config
        .providers
        .into_iter()
        .filter(|provider| provider.enabled)
        .collect::<Vec<_>>();
    if providers.is_empty() {
        bail!("provider config must enable at least one provider");
    }
    validate_providers(&providers)?;
    let main_model = providers
        .iter()
        .find(|provider| provider.id == config.main_provider)
        .map(|provider| provider.default_model.clone())
        .context("mainProvider must name an enabled provider")?;
    let routes = providers.into_iter().map(Provider::into_route).collect();
    Ok(LoadedConfig { main_model, routes })
}

fn validate_choice(choice: &AgentChoice, name: &str) -> Result<()> {
    if [&choice.agent, &choice.model, &choice.effort]
        .into_iter()
        .any(|value| value.is_empty())
    {
        bail!("provider config {name} fields must not be empty");
    }
    Ok(())
}

fn validate_providers(providers: &[Provider]) -> Result<()> {
    let mut ids = HashSet::new();
    let mut models = HashSet::new();
    let mut prefixes = HashSet::new();
    for provider in providers {
        if provider
            .required_fields()
            .iter()
            .any(|value| value.is_empty())
        {
            bail!("enabled provider fields must not be empty");
        }
        if !ids.insert(&provider.id) {
            bail!("enabled provider IDs must be unique");
        }
        if !models.insert(&provider.default_model) {
            bail!("enabled provider defaultModel values must be unique");
        }
        if provider.model_prefixes.iter().any(String::is_empty) {
            bail!("modelPrefixes must not contain an empty value");
        }
        if !provider
            .model_prefixes
            .iter()
            .all(|prefix| prefixes.insert(prefix))
        {
            bail!("enabled provider modelPrefixes must be unique");
        }
        validate_acp(provider)?;
    }
    Ok(())
}

fn validate_acp(provider: &Provider) -> Result<()> {
    match (provider.backend, &provider.acp) {
        (BackendKind::ConfiguredAcp, Some(acp))
            if !acp.program.is_empty() && !acp.arguments.is_empty() =>
        {
            Ok(())
        }
        (BackendKind::ConfiguredAcp, _) => {
            bail!("configured-acp requires a non-empty acp program and arguments")
        }
        (_, None) => Ok(()),
        (_, Some(_)) => bail!("acp is valid only with configured-acp"),
    }
}

impl Provider {
    fn required_fields(&self) -> [&str; 4] {
        [&self.id, &self.agent, &self.default_model, &self.effort]
    }

    fn into_route(self) -> BackendRoute {
        let _ = self.usage_provider;
        BackendRoute {
            model: self.default_model,
            backend: self.backend,
            model_prefixes: self.model_prefixes,
            acp: self.acp,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn config(provider: &str) -> String {
        format!(
            r#"{{"version":1,"mainProvider":"p","providers":[{provider}],"fallback":{{"agent":"f","model":"m","effort":"high"}},"advisor":{{"agent":"a","model":"x","effort":"xhigh"}}}}"#
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
                r#"{"id":"p","agent":"worker","defaultModel":"model","effort":"high","enabled":true,"usageProvider":"quota","modelPrefixes":["model-"],"backend":"codex-app-server"},{"id":"off","agent":"off","defaultModel":"off","effort":"low","enabled":false,"backend":"grok-acp"}"#,
            ),
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.main_model, "model");
        assert_eq!(loaded.routes.len(), 1);
        assert_eq!(loaded.routes[0].model_prefixes, ["model-"]);
    }

    #[test]
    fn accepts_a_configured_acp() {
        let json = config(
            r#"{"id":"p","agent":"worker","defaultModel":"new-1","effort":"high","enabled":true,"modelPrefixes":["new-"],"backend":"configured-acp","acp":{"program":"new-acp","arguments":["--model","{model}","--stdio"]}}"#,
        );
        let parsed: ProviderConfig = serde_json::from_str(&json).unwrap();
        let loaded = validate(parsed).unwrap();
        assert_eq!(loaded.routes[0].acp.as_ref().unwrap().program, "new-acp");
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
        config.main_provider = "missing".to_owned();
        invalid.push(config);
        let mut config = parsed();
        config.fallback.agent.clear();
        invalid.push(config);
        let mut config = parsed();
        config.providers[0].model_prefixes = vec![String::new()];
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
}
