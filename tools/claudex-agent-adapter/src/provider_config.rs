use crate::agent_backend::{AcpLaunch, BackendKind, BackendRoute};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{collections::HashSet, fs, path::Path};
const CONFIG_VERSION: u64 = 1;
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ProviderConfig {
    version: u64,
    main_providers: Vec<String>,
    providers: Vec<Provider>,
    fallback: AgentChoice,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Provider {
    id: String,
    agent: String,
    default_model: String,
    #[serde(default)]
    subagent_model: Option<String>,
    effort: String,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    #[serde(default)]
    usage_provider: Option<String>,
    #[serde(default)]
    model_provider: Option<String>,
    #[serde(default)]
    model_catalog_json: Option<String>,
    #[serde(default)]
    max_context_tokens: Option<u64>,
    #[serde(default)]
    max_concurrency: Option<usize>,
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
    /// Exact default models and prefixes declared by any provider entry, including disabled ones.
    /// Used to remap unrouted provider ids onto the main backend without hardcoding vendor names.
    pub model_catalog: ModelCatalog,
}
/// Config-declared model identities used for routing remaps.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ModelCatalog {
    exact: Vec<String>,
    prefixes: Vec<String>,
}
impl ModelCatalog {
    fn from_providers<'a>(providers: impl IntoIterator<Item = &'a Provider>) -> Self {
        let mut exact = Vec::new();
        let mut prefixes = Vec::new();
        for provider in providers {
            if !provider.default_model.is_empty() {
                exact.push(provider.default_model.clone());
            }
            if let Some(model) = provider
                .subagent_model
                .as_ref()
                .filter(|model| !model.is_empty())
            {
                exact.push(model.clone());
            }
            prefixes.extend(
                provider
                    .model_prefixes
                    .iter()
                    .filter(|prefix| !prefix.is_empty())
                    .cloned(),
            );
        }
        exact.sort();
        exact.dedup();
        prefixes.sort();
        prefixes.dedup();
        Self { exact, prefixes }
    }
    pub fn from_routes(routes: &[BackendRoute]) -> Self {
        let mut exact = Vec::new();
        let mut prefixes = Vec::new();
        for route in routes {
            if !route.model.is_empty() {
                exact.push(route.model.clone());
            }
            prefixes.extend(
                route
                    .model_prefixes
                    .iter()
                    .filter(|prefix| !prefix.is_empty())
                    .cloned(),
            );
        }
        exact.sort();
        exact.dedup();
        prefixes.sort();
        prefixes.dedup();
        Self { exact, prefixes }
    }
    pub fn matches(&self, model: &str) -> bool {
        self.exact.iter().any(|exact| exact == model)
            || self
                .prefixes
                .iter()
                .any(|prefix| model.starts_with(prefix.as_str()))
    }
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
    // Keep identities for disabled providers so exhausted/denied backends can still be
    // recognized and remapped instead of falling through to Claude subscription under a
    // stale provider model id.
    let model_catalog = ModelCatalog::from_providers(&config.providers);
    let providers = config
        .providers
        .into_iter()
        .filter(|provider| provider.enabled)
        .collect::<Vec<_>>();
    if providers.is_empty() {
        bail!("provider config must enable at least one provider");
    }
    validate_providers(&providers)?;
    let enabled_ids = providers
        .iter()
        .map(|provider| provider.id.as_str())
        .collect::<HashSet<_>>();
    let main_ids = config
        .main_providers
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if main_ids.is_empty()
        || main_ids.len() != config.main_providers.len()
        || !main_ids.is_subset(&enabled_ids)
    {
        bail!("mainProviders must name distinct enabled providers");
    }
    let main_model = providers
        .iter()
        .find(|provider| provider.id == config.main_providers[0])
        .expect("validated main provider")
        .default_model
        .clone();
    let routes = providers.into_iter().map(Provider::into_route).collect();
    Ok(LoadedConfig {
        main_model,
        routes,
        model_catalog,
    })
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
        if provider.max_context_tokens == Some(0) {
            bail!("maxContextTokens must be greater than zero");
        }
        if provider
            .max_concurrency
            .is_some_and(|limit| limit == 0 || limit > crate::grok_acp::MAX_MODEL_CONCURRENCY)
        {
            bail!("maxConcurrency must be between 1 and the adapter semaphore limit");
        }
        if provider
            .subagent_model
            .as_ref()
            .is_some_and(String::is_empty)
        {
            bail!("subagentModel must not be empty");
        }
        if provider
            .model_provider
            .as_ref()
            .is_some_and(String::is_empty)
            || provider
                .model_catalog_json
                .as_ref()
                .is_some_and(String::is_empty)
        {
            bail!("modelProvider and modelCatalogJson must not be empty");
        }
        if provider.backend != BackendKind::CodexAppServer
            && (provider.model_provider.is_some() || provider.model_catalog_json.is_some())
        {
            bail!("modelProvider and modelCatalogJson are valid only with codex-app-server");
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
            model_provider: self.model_provider,
            model_catalog_json: self.model_catalog_json,
            max_context_tokens: self.max_context_tokens,
            max_concurrency: self.max_concurrency,
            model_prefixes: self.model_prefixes,
            acp: self.acp,
        }
    }
}
#[cfg(test)]
include!("provider_config_tests.rs");
