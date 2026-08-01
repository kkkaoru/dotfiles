use crate::agent_backend::{AcpLaunch, BackendKind, BackendRoute, WebSearchMode};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::Path};

mod validation;
use validation::{validate_choice, validate_providers, validate_worker_routes};
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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct WebSearchSettings {
    #[serde(default)]
    fallback_providers: Vec<String>,
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
    request_budget: Option<RequestBudget>,
    #[serde(default)]
    model_prefixes: Vec<String>,
    backend: BackendKind,
    #[serde(default)]
    acp: Option<AcpLaunch>,
    #[serde(default)]
    web_search_mode: WebSearchMode,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(dead_code)]
struct RequestBudget {
    estimated_requests: u64,
    window_minutes: u64,
    usage_window: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentChoice {
    agent: String,
    model: String,
    effort: String,
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
    exact: Vec<String>,
    prefixes: Vec<String>,
    workers: Vec<WorkerRoute>,
    search_workers: Vec<WorkerRoute>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkerRoute {
    pub agent: String,
    pub model: String,
    pub effort: String,
}
impl ModelCatalog {
    fn from_providers<'a>(providers: impl IntoIterator<Item = &'a Provider>) -> Self {
        let mut exact = Vec::new();
        let mut prefixes = Vec::new();
        for provider in providers {
            collect_provider_models(provider, &mut exact, &mut prefixes);
        }
        exact.sort();
        exact.dedup();
        prefixes.sort();
        prefixes.dedup();
        Self {
            exact,
            prefixes,
            workers: Vec::new(),
            search_workers: Vec::new(),
        }
    }
    pub fn from_routes(routes: &[BackendRoute]) -> Self {
        let mut exact = Vec::new();
        let mut prefixes = Vec::new();
        for route in routes {
            collect_route_models(route, &mut exact, &mut prefixes);
        }
        exact.sort();
        exact.dedup();
        prefixes.sort();
        prefixes.dedup();
        Self {
            exact,
            prefixes,
            workers: Vec::new(),
            search_workers: Vec::new(),
        }
    }
    pub fn matches(&self, model: &str) -> bool {
        self.exact.iter().any(|exact| exact == model)
            || self
                .prefixes
                .iter()
                .any(|prefix| model.starts_with(prefix.as_str()))
    }

    pub fn worker_fields(&self, agent: &str) -> Option<(&str, &str)> {
        self.workers
            .iter()
            .find(|worker| worker.agent == agent)
            .map(|worker| (worker.model.as_str(), worker.effort.as_str()))
    }

    pub fn worker_effort_for_model(&self, model: &str) -> Option<&str> {
        self.workers
            .iter()
            .chain(self.search_workers.iter())
            .find(|worker| worker.model == model)
            .map(|worker| worker.effort.as_str())
    }

    pub fn worker_routes(&self) -> &[WorkerRoute] {
        &self.workers
    }

    pub fn search_worker_routes(&self) -> &[WorkerRoute] {
        &self.search_workers
    }

    pub fn with_search_worker_routes(mut self, workers: Vec<WorkerRoute>) -> Result<Self> {
        self.set_search_worker_routes(workers)?;
        Ok(self)
    }

    pub fn set_search_worker_routes(&mut self, workers: Vec<WorkerRoute>) -> Result<()> {
        validate_worker_routes(&workers)?;
        self.search_workers = workers;
        Ok(())
    }

    pub fn set_worker_routes(&mut self, workers: Vec<WorkerRoute>) -> Result<()> {
        validate_worker_routes(&workers)?;
        self.workers = workers;
        Ok(())
    }

    fn add_workers(
        &mut self,
        providers: &[Provider],
        native_workers: &[WorkerRoute],
    ) -> Result<()> {
        let mut workers = providers
            .iter()
            .map(|provider| WorkerRoute {
                agent: provider.agent.clone(),
                model: provider
                    .subagent_model
                    .as_ref()
                    .unwrap_or(&provider.default_model)
                    .clone(),
                effort: provider.effort.clone(),
            })
            .collect::<Vec<_>>();
        workers.extend_from_slice(native_workers);
        self.set_worker_routes(workers)
    }
}

fn collect_provider_models(
    provider: &Provider,
    exact: &mut Vec<String>,
    prefixes: &mut Vec<String>,
) {
    push_nonempty(exact, &provider.default_model);
    if let Some(model) = provider.subagent_model.as_deref() {
        push_nonempty(exact, model);
    }
    extend_nonempty(prefixes, &provider.model_prefixes);
}

fn collect_route_models(route: &BackendRoute, exact: &mut Vec<String>, prefixes: &mut Vec<String>) {
    push_nonempty(exact, &route.model);
    extend_nonempty(prefixes, &route.model_prefixes);
}

fn push_nonempty(values: &mut Vec<String>, value: &str) {
    if !value.is_empty() {
        values.push(value.to_owned());
    }
}

fn extend_nonempty(values: &mut Vec<String>, candidates: &[String]) {
    values.extend(candidates.iter().filter(|value| !value.is_empty()).cloned());
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
    // Keep identities for disabled providers so exhausted/denied backends can still be
    // recognized and remapped instead of falling through to Claude subscription under a
    // stale provider model id.
    let mut model_catalog = ModelCatalog::from_providers(&config.providers);
    let search_provider_ids = config.web_search.fallback_providers.clone();
    let providers = config
        .providers
        .into_iter()
        .filter(|provider| provider.enabled)
        .collect::<Vec<_>>();
    if providers.is_empty() {
        bail!("provider config must enable at least one provider");
    }
    validate_providers(&providers)?;
    model_catalog.add_workers(&providers, &config.native_workers)?;
    let search_workers = search_provider_ids
        .iter()
        .map(|id| {
            providers
                .iter()
                .find(|provider| &provider.id == id)
                .with_context(|| format!("webSearch fallback provider `{id}` is not enabled"))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .map(|provider| WorkerRoute {
            agent: provider.agent.clone(),
            model: provider
                .subagent_model
                .as_ref()
                .unwrap_or(&provider.default_model)
                .clone(),
            effort: provider.effort.clone(),
        })
        .collect();
    model_catalog.set_search_worker_routes(search_workers)?;
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
    let routes = providers.into_iter().map(Provider::into_route).collect();
    Ok(LoadedConfig {
        routes,
        model_catalog,
    })
}
impl Provider {
    fn required_fields(&self) -> [&str; 4] {
        [&self.id, &self.agent, &self.default_model, &self.effort]
    }
    fn into_route(self) -> BackendRoute {
        let _ = self.usage_provider;
        let _ = self.request_budget.map(|budget| {
            (
                budget.estimated_requests,
                budget.window_minutes,
                budget.usage_window,
            )
        });
        BackendRoute {
            model: self.default_model,
            backend: self.backend,
            effort: Some(self.effort),
            model_provider: self.model_provider,
            model_catalog_json: self.model_catalog_json,
            max_context_tokens: self.max_context_tokens,
            max_concurrency: self.max_concurrency,
            model_prefixes: self.model_prefixes,
            acp: self.acp,
            web_search_mode: self.web_search_mode,
        }
    }
}
#[cfg(test)]
include!("provider_config_tests.rs");
