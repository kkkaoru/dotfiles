use super::MessagesRequest;
use anyhow::{Result, bail};

mod models;
use models::normalize_claude_model_to_haiku;
pub(in crate::anthropic) use models::official_claude_haiku_model;

/// Apply SubAgent intent overrides and policy denylist / unrouted-provider remaps.
#[cfg(test)]
pub(super) fn resolve_request_model(
    request: &mut MessagesRequest,
    main_model: &str,
    is_subagent: bool,
    intent_matched: bool,
    model_override: Option<String>,
    supports_model: impl Fn(&str) -> bool,
    // True when the model matches any provider identity declared in config (enabled or not).
    is_declared_provider_model: impl Fn(&str) -> bool,
) -> Result<RouteDecision> {
    resolve_request_model_with_origin(
        request,
        main_model,
        is_subagent,
        intent_matched,
        model_override,
        false,
        supports_model,
        is_declared_provider_model,
    )
}

/// Resolve a request while retaining whether a SubAgent model came from its parent.
/// Explicit child model selections are never rewritten.
pub(super) fn resolve_request_model_with_origin(
    request: &mut MessagesRequest,
    main_model: &str,
    is_subagent: bool,
    intent_matched: bool,
    model_override: Option<String>,
    model_is_inherited: bool,
    supports_model: impl Fn(&str) -> bool,
    // True when the model matches any provider identity declared in config (enabled or not).
    is_declared_provider_model: impl Fn(&str) -> bool,
) -> Result<RouteDecision> {
    resolve_request_model_inner(
        request,
        main_model,
        is_subagent,
        intent_matched,
        model_override,
        model_is_inherited,
        &supports_model,
        &is_declared_provider_model,
    )
}

fn resolve_request_model_inner(
    request: &mut MessagesRequest,
    main_model: &str,
    is_subagent: bool,
    intent_matched: bool,
    model_override: Option<String>,
    model_is_inherited: bool,
    supports_model: &dyn Fn(&str) -> bool,
    is_declared_provider_model: &dyn Fn(&str) -> bool,
) -> Result<RouteDecision> {
    if is_subagent && (!intent_matched || model_override.is_none()) {
        tracing::warn!(
            request_model = %request.model,
            intent_matched,
            has_explicit_model = model_override.is_some(),
            "routing a SubAgent from its request model because launch metadata is incomplete"
        );
    }
    let has_model_override = model_override.is_some();
    if let Some(model) = model_override {
        request.model = model;
    }
    if let Some(model) = request
        .model
        .strip_prefix(crate::DISCOVERY_MODEL_PREFIX)
        .filter(|model| *model == main_model || supports_model(model))
        .map(str::to_owned)
    {
        request.model = model;
    }
    if is_subagent
        && (!has_model_override || model_is_inherited)
        && let Some(model) = normalize_claude_model_to_haiku(&request.model)
    {
        tracing::warn!(
            request_model = %request.model,
            normalized_model = model,
            "routing a Claude SubAgent tool request through the small fast model"
        );
        request.model = model.to_owned();
        return Ok(RouteDecision::Subscription);
    }

    apply_disabled_model_policy(request, main_model, is_subagent)?;

    let explicit_native_claude = has_model_override
        && !model_is_inherited
        && normalize_claude_model_to_haiku(&request.model).is_some();
    if is_subagent
        && !explicit_native_claude
        && (request.model.is_empty()
            || (request.model != main_model && !supports_model(&request.model)))
    {
        bail!(
            "SubAgent model `{}` does not have a recoverable configured route and must not be launched",
            request.model
        );
    }

    if request.model.is_empty() || request.model == main_model || supports_model(&request.model) {
        return Ok(RouteDecision::Provider);
    }
    if is_declared_provider_model(&request.model) {
        tracing::warn!(
            request_model = %request.model,
            main_model,
            "remapping request for an unrouted provider model onto the adapter main route"
        );
        request.model = main_model.to_owned();
        return Ok(RouteDecision::Provider);
    }
    Ok(RouteDecision::Subscription)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RouteDecision {
    Provider,
    Subscription,
}

fn apply_disabled_model_policy(
    request: &mut MessagesRequest,
    main_model: &str,
    is_subagent: bool,
) -> Result<()> {
    if !request.disabled_subagent_models.contains(&request.model) {
        return Ok(());
    }
    if is_subagent {
        bail!(
            "SubAgent model `{}` is disabled by the active Claudex policy and must not be launched",
            request.model
        );
    }
    if request.model == main_model {
        // The dedicated policy controls only spawned workers. A terminal may
        // keep using a model in its outer session while denying that same
        // model to SubAgents.
        return Ok(());
    }
    tracing::warn!(
        disabled_model = %request.model,
        main_model,
        "remapping outer request off a policy-disabled model onto the adapter main route"
    );
    request.model = main_model.to_owned();
    Ok(())
}

#[cfg(test)]
include!("request_routing_tests.rs");
