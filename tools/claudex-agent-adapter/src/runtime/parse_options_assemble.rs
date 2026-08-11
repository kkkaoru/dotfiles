use anyhow::{Result, bail};

use crate::{
    agent_backend::{BackendKind, BackendRoute},
    launcher::AdapterOptions,
    provider_config,
};

use super::{OptionsDraft, ParsedOptions, hard_timeout};

pub(super) fn assemble_options(mut draft: OptionsDraft) -> Result<ParsedOptions> {
    validate_limits(draft.max_processes, draft.timeout_minutes)?;
    let hard_timeout = hard_timeout::resolve(draft.hard_timeout_cli)?;
    let configured = draft
        .provider_config
        .as_deref()
        .map(provider_config::load)
        .transpose()?;
    let has_provider_config = configured.is_some();
    let mut model_catalog = configured
        .as_ref()
        .map(|configured| configured.model_catalog.clone())
        .unwrap_or_default();
    if let Some(configured) = &configured {
        draft.routes.splice(0..0, configured.routes.clone());
    }
    if draft.model.as_deref().is_some_and(str::is_empty) {
        bail!("--model must not be empty");
    }
    let model = draft.model.unwrap_or_default();
    if model.is_empty() && !has_provider_config && draft.routes.is_empty() {
        bail!("--model or --provider-config is required");
    }
    if draft.routes.is_empty() {
        draft
            .routes
            .push(BackendRoute::new(&model, BackendKind::CodexAppServer));
    }
    if model_catalog == provider_config::ModelCatalog::default() {
        model_catalog = provider_config::ModelCatalog::from_routes(&draft.routes);
    }
    if !draft.worker_routes.is_empty() {
        model_catalog.set_worker_routes(draft.worker_routes)?;
    }
    if !draft.search_worker_routes.is_empty() {
        model_catalog.set_search_worker_routes(draft.search_worker_routes)?;
    }
    if !draft.selectable_models.is_empty() {
        model_catalog.set_selectable_models(draft.selectable_models);
    }
    validate_routes(&draft.routes)?;
    Ok(ParsedOptions {
        adapter: AdapterOptions {
            routes: draft.routes,
            model,
            listen: draft.listen,
            subscription_max_processes: draft.max_processes,
            subscription_timeout_minutes: draft.timeout_minutes,
            subagent_hard_timeout_seconds: hard_timeout,
            model_catalog,
        },
        inherit_claude_model: draft.inherit_claude_model,
    })
}
fn validate_routes(routes: &[BackendRoute]) -> Result<()> {
    if routes.iter().any(|route| {
        route
            .max_concurrency
            .is_some_and(|limit| limit == 0 || limit > crate::grok_acp::MAX_MODEL_CONCURRENCY)
    }) {
        bail!("backend route maxConcurrency is out of range");
    }
    let unique = routes
        .iter()
        .map(|route| route.model.as_str())
        .collect::<std::collections::HashSet<_>>();
    if unique.len() != routes.len() {
        bail!("--backend-route models must be unique");
    }
    Ok(())
}

fn validate_limits(max_processes: usize, timeout_minutes: u64) -> Result<()> {
    if max_processes > tokio::sync::Semaphore::MAX_PERMITS {
        bail!("--subscription-max-processes is out of range");
    }
    if timeout_minutes.checked_mul(60).is_none() {
        bail!("--subscription-timeout-minutes is out of range");
    }
    Ok(())
}
