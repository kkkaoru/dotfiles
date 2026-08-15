use crate::agent_backend::{AcpLaunch, BackendKind, BackendRoute, WebSearchMode};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

mod catalog;
mod identities;
mod types;
mod validation;
mod worker_route;
use types::{AgentChoice, RequestBudget, WebSearchSettings};
use validation::{
    auxiliary_worker_routes, search_workers_from_providers, validate_choice,
    validate_main_providers, validate_providers, validate_worker_routes,
};
const CONFIG_VERSION: u64 = 1;
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ProviderConfig {
    version: u64,
    main_providers: Vec<String>,
    providers: Vec<Provider>,
    fallback: AgentChoice,
    #[serde(default)]
    native_workers: Vec<WorkerRoute>,
    #[serde(default)]
    advisor: Option<AgentChoice>,
    #[serde(default)]
    web_search: WebSearchSettings,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Provider {
    pub(super) id: String,
    pub(super) agent: String,
    pub(super) default_model: String,
    #[serde(default)]
    pub(super) subagent_model: Option<String>,
    pub(super) effort: String,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    #[serde(default)]
    pub(super) usage_provider: Option<String>,
    #[serde(default)]
    usage_weekly_window_id: Option<String>,
    #[serde(default)]
    model_provider: Option<String>,
    #[serde(default)]
    model_catalog_json: Option<String>,
    #[serde(default)]
    pi_provider: Option<String>,
    #[serde(default)]
    pi_model: Option<String>,
    #[serde(default)]
    max_context_tokens: Option<u64>,
    #[serde(default)]
    max_concurrency: Option<usize>,
    #[serde(default)]
    request_budget: Option<RequestBudget>,
    #[serde(default)]
    pub(super) model_prefixes: Vec<String>,
    #[serde(default)]
    pub(super) selectable_models: Vec<String>,
    backend: BackendKind,
    #[serde(default)]
    acp: Option<AcpLaunch>,
    #[serde(default)]
    web_search_mode: WebSearchMode,
}

pub struct LoadedConfig {
    pub routes: Vec<BackendRoute>,
    /// Exact default models and prefixes declared by any provider entry, including disabled ones.
    /// Used to remap unrouted provider ids onto the main backend without hardcoding vendor names.
    pub model_catalog: ModelCatalog,
}
/// Config-declared model identities used for routing remaps.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ModelCatalog {
    pub(super) exact: Vec<String>,
    pub(super) prefixes: Vec<String>,
    pub(super) selectable: Vec<String>,
    pub(super) workers: Vec<WorkerRoute>,
    pub(super) search_workers: Vec<WorkerRoute>,
    // Explicit Claude fallback, generic Haiku, and custom-advisor routes are
    // valid routing identities but are not capacity-managed daemon workers.
    pub(super) auxiliary_workers: Vec<WorkerRoute>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerRoute {
    pub agent: String,
    pub model: String,
    pub effort: String,
    /// Quota association for routing hooks; ignored by the adapter daemon.
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    usage_provider: Option<String>,
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
    validate_worker_routes(&config.native_workers)?;
    if let Some(advisor) = config.advisor.as_ref() {
        validate_choice(advisor, "advisor")?;
    }
    let search_provider_ids = config.web_search.fallback_providers.clone();
    let (providers, mut model_catalog) = enabled_providers_catalog(config.providers)?;
    model_catalog.add_workers(&providers, &config.native_workers)?;
    model_catalog.set_auxiliary_worker_routes(auxiliary_worker_routes(
        &config.fallback,
        config.advisor.as_ref(),
    ))?;
    model_catalog.set_search_worker_routes(search_workers_from_providers(
        &providers,
        &search_provider_ids,
    )?)?;
    validate_main_providers(&providers, &config.main_providers)?;
    let routes = providers.into_iter().map(Provider::into_route).collect();
    Ok(LoadedConfig {
        routes,
        model_catalog,
    })
}
fn enabled_providers_catalog(providers: Vec<Provider>) -> Result<(Vec<Provider>, ModelCatalog)> {
    // Keep identities for disabled providers so exhausted/denied backends can still be
    // recognized and remapped instead of falling through to Claude subscription.
    let model_catalog = ModelCatalog::from_providers(&providers);
    let providers = providers
        .into_iter()
        .filter(|provider| provider.enabled)
        .collect::<Vec<_>>();
    if providers.is_empty() {
        bail!("provider config must enable at least one provider");
    }
    validate_providers(&providers)?;
    Ok((providers, model_catalog))
}

#[cfg(test)]
impl Provider {
    fn listed_models(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.default_model.as_str()).chain(self.subagent_model.as_deref())
    }

    fn conflicts_hostname_denylist(&self, denylist: &std::collections::BTreeSet<String>) -> bool {
        self.enabled && self.listed_models().any(|model| denylist.contains(model))
    }
}

#[cfg(test)]
fn enabled_denylist_conflicts(
    providers: &[Provider],
    denylist: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    providers
        .iter()
        .filter(|provider| provider.conflicts_hostname_denylist(denylist))
        .map(|provider| provider.id.clone())
        .collect()
}

mod provider_route;

#[cfg(test)]
include!("provider_config_tests.rs");
