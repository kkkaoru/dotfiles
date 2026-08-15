use anyhow::{Result, bail};
use std::collections::HashSet;

use super::{BackendKind, Provider, WebSearchMode};

pub(super) fn validate_effort(provider: &Provider) -> Result<()> {
    if provider.backend == BackendKind::GrokAcp
        && !matches!(provider.effort.as_str(), "low" | "medium" | "high")
    {
        bail!("grok-acp effort must be one of low, medium, or high");
    }
    Ok(())
}

pub(super) fn validate_identity(
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

pub(super) fn validate_limits(provider: &Provider) -> Result<()> {
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

pub(super) fn validate_backend_fields(provider: &Provider) -> Result<()> {
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

pub(super) fn validate_acp(provider: &Provider) -> Result<()> {
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

pub(super) fn validate_claude_models_are_not_pi_routes(provider: &Provider) -> Result<()> {
    if provider.pi_provider.is_none() {
        return Ok(());
    }
    let mut models = std::iter::once(provider.default_model.as_str())
        .chain(provider.subagent_model.as_deref())
        .chain(provider.selectable_models.iter().map(String::as_str));
    if let Some(model) =
        models.find(|model| crate::anthropic::normalize_claude_model_to_haiku(model).is_some())
    {
        bail!(
            "provider {} maps Claude model `{model}` through Pi; Claude models must not be routed through pi",
            provider.id
        );
    }
    if let Some(prefix) = provider
        .model_prefixes
        .iter()
        .find(|prefix| prefix_captures_claude_model(prefix))
    {
        bail!(
            "provider {} maps Claude model prefix `{prefix}` through Pi; Claude models must not be routed through pi",
            provider.id
        );
    }
    Ok(())
}

fn prefix_captures_claude_model(prefix: &str) -> bool {
    if prefix.is_empty() || prefix.starts_with(crate::DISCOVERY_MODEL_PREFIX) {
        return false;
    }
    const SAMPLES: [&str; 8] = [
        "claude-haiku-4-5",
        "claude-sonnet-5",
        "claude-opus-5",
        "haiku",
        "sonnet",
        "opus",
        "fable",
        "sonnet[1m]",
    ];
    crate::anthropic::normalize_claude_model_to_haiku(prefix).is_some()
        || SAMPLES.iter().any(|sample| sample.starts_with(prefix))
}

pub(super) fn validate_web_search_mode(provider: &Provider) -> Result<()> {
    let valid = match provider.web_search_mode {
        WebSearchMode::CodexNative => provider.backend == BackendKind::CodexAppServer,
        WebSearchMode::AcpNative | WebSearchMode::DelegateMcp => {
            provider.backend != BackendKind::CodexAppServer
        }
        WebSearchMode::DelegateCcr | WebSearchMode::DelegatePi | WebSearchMode::Disabled => true,
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
