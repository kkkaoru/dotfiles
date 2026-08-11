use anyhow::{Context, Result, bail};
use std::collections::HashSet;

use super::{AgentChoice, BackendKind, Provider, WebSearchMode, WorkerRoute};

pub(super) fn validate_worker_routes(workers: &[WorkerRoute]) -> Result<()> {
    if workers.iter().any(|worker| {
        [
            worker.agent.as_str(),
            worker.model.as_str(),
            worker.effort.as_str(),
        ]
        .into_iter()
        .any(str::is_empty)
    }) {
        bail!("worker route fields must not be empty");
    }
    let agents = workers
        .iter()
        .map(|worker| worker.agent.as_str())
        .collect::<HashSet<_>>();
    if agents.len() != workers.len() {
        bail!("worker route agent values must be unique");
    }
    Ok(())
}

pub(super) fn validate_choice(choice: &AgentChoice, name: &str) -> Result<()> {
    if [&choice.agent, &choice.model, &choice.effort]
        .into_iter()
        .any(|value| value.is_empty())
    {
        bail!("provider config {name} fields must not be empty");
    }
    Ok(())
}

pub(super) fn validate_providers(providers: &[Provider]) -> Result<()> {
    let mut ids = HashSet::new();
    let mut agents = HashSet::new();
    let mut models = HashSet::new();
    let mut prefixes = HashSet::new();
    for provider in providers {
        validate_identity(provider, &mut ids, &mut agents, &mut models)?;
        validate_limits(provider)?;
        validate_backend_fields(provider)?;
        validate_effort(provider)?;
        validate_web_search_mode(provider)?;
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

fn validate_effort(provider: &Provider) -> Result<()> {
    if provider.backend == BackendKind::GrokAcp
        && !matches!(provider.effort.as_str(), "low" | "medium" | "high")
    {
        bail!("grok-acp effort must be one of low, medium, or high");
    }
    Ok(())
}

fn validate_identity(
    provider: &Provider,
    ids: &mut HashSet<String>,
    agents: &mut HashSet<String>,
    models: &mut HashSet<String>,
) -> Result<()> {
    if provider
        .required_fields()
        .iter()
        .any(|value| value.is_empty())
    {
        bail!("enabled provider fields must not be empty");
    }
    if !ids.insert(provider.id.clone()) {
        bail!("enabled provider IDs must be unique");
    }
    if !agents.insert(provider.agent.clone()) {
        bail!("enabled provider agent values must be unique");
    }
    if !models.insert(provider.default_model.clone()) {
        bail!("enabled provider defaultModel values must be unique");
    }
    if provider.model_prefixes.iter().any(String::is_empty) {
        bail!("modelPrefixes must not contain an empty value");
    }
    if provider.selectable_models.iter().any(String::is_empty) {
        bail!("selectableModels must not contain an empty value");
    }
    Ok(())
}

fn validate_limits(provider: &Provider) -> Result<()> {
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
    Ok(())
}

fn validate_backend_fields(provider: &Provider) -> Result<()> {
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

fn validate_web_search_mode(provider: &Provider) -> Result<()> {
    let valid = match provider.web_search_mode {
        WebSearchMode::CodexNative => provider.backend == BackendKind::CodexAppServer,
        WebSearchMode::AcpNative | WebSearchMode::DelegateMcp => {
            provider.backend != BackendKind::CodexAppServer
        }
        WebSearchMode::DelegateCcr | WebSearchMode::Disabled => true,
    };
    if !valid {
        bail!(
            "webSearchMode `{}` is incompatible with backend `{}`",
            provider.web_search_mode,
            provider.backend
        );
    }
    Ok(())
}

pub(super) fn auxiliary_worker_routes(
    fallback: &super::AgentChoice,
    advisor: Option<&super::AgentChoice>,
) -> Vec<super::WorkerRoute> {
    let mut auxiliary_workers = vec![
        super::WorkerRoute::new(
            fallback.agent.clone(),
            fallback.model.clone(),
            fallback.effort.clone(),
        ),
        super::WorkerRoute::new(
            "claudex-haiku",
            crate::anthropic::official_claude_haiku_model(),
            "max",
        ),
    ];
    if let Some(advisor) = advisor {
        auxiliary_workers.push(super::WorkerRoute::new(
            advisor.agent.clone(),
            advisor.model.clone(),
            advisor.effort.clone(),
        ));
    }
    auxiliary_workers
}

pub(super) fn search_workers_from_providers(
    providers: &[super::Provider],
    search_provider_ids: &[String],
) -> Result<Vec<super::WorkerRoute>> {
    let selected = search_provider_ids
        .iter()
        .map(|id| {
            providers
                .iter()
                .find(|provider| &provider.id == id)
                .with_context(|| format!("webSearch fallback provider `{id}` is not enabled"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(selected
        .into_iter()
        .map(|provider| {
            super::WorkerRoute::new(
                provider.agent.clone(),
                provider
                    .subagent_model
                    .as_ref()
                    .unwrap_or(&provider.default_model)
                    .clone(),
                provider.effort.clone(),
            )
            .with_usage_provider(provider.usage_provider.clone())
        })
        .collect())
}

pub(super) fn validate_main_providers(
    providers: &[super::Provider],
    main_providers: &[String],
) -> Result<()> {
    let enabled_ids = providers
        .iter()
        .map(|provider| provider.id.as_str())
        .collect::<HashSet<_>>();
    let main_ids = main_providers
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if main_ids.is_empty()
        || main_ids.len() != main_providers.len()
        || !main_ids.is_subset(&enabled_ids)
    {
        bail!("mainProviders must name distinct enabled providers");
    }
    Ok(())
}
