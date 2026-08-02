use super::{MessagesRequest, content::serialized_len};
use anyhow::{Result, bail};

mod models;
pub(crate) use models::official_claude_haiku_model;
use models::{CLAUDE_LONG_CONTEXT_MODEL, normalize_claude_model_to_haiku};

// Claude Haiku 4.5 has a 200k context window, but a subscription child also
// receives Claude's system prompt, tool definitions, and attachments. Keep a
// 100k conversation budget so a resumed parent cannot repeatedly launch an
// oversized Haiku child. The observed failure was 117k conversation tokens and
// 210k total request tokens.
const HAIKU_CONVERSATION_TOKEN_BUDGET: usize = 100_000;

/// Apply SubAgent intent overrides and policy denylist / unrouted-provider remaps.
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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
        model_override,
        RouteOrigin::new(is_subagent, intent_matched, false),
        supports_model,
        is_declared_provider_model,
    )
}

/// Resolve a request while retaining whether a SubAgent model came from its parent.
/// Explicit child model selections are never rewritten.
pub(super) fn resolve_request_model_with_origin(
    request: &mut MessagesRequest,
    main_model: &str,
    model_override: Option<String>,
    origin: RouteOrigin,
    supports_model: impl Fn(&str) -> bool,
    // True when the model matches any provider identity declared in config (enabled or not).
    is_declared_provider_model: impl Fn(&str) -> bool,
) -> Result<RouteDecision> {
    if origin.is_subagent && (!origin.intent_matched || model_override.is_none()) {
        tracing::warn!(
            request_model = %request.model,
            intent_matched = origin.intent_matched,
            has_explicit_model = model_override.is_some(),
            "routing a SubAgent from its request model because launch metadata is incomplete"
        );
    }
    let has_model_override = model_override.is_some();
    if let Some(model) = model_override {
        request.model = model;
    }
    if request.model.is_empty() {
        bail!("request model is required; the adapter does not select a provider main model");
    }
    if let Some(model) = request
        .model
        .strip_prefix(crate::DISCOVERY_MODEL_PREFIX)
        .filter(|model| *model == main_model || supports_model(model))
        .map(str::to_owned)
    {
        request.model = model;
    }
    if origin.is_subagent
        && (!has_model_override || origin.model_is_inherited)
        && request.model == CLAUDE_LONG_CONTEXT_MODEL
    {
        return Ok(RouteDecision::Subscription);
    }
    if origin.is_subagent
        && (!has_model_override || origin.model_is_inherited)
        && let Some(model) = normalize_claude_model_to_haiku(&request.model)
    {
        if conversation_exceeds_haiku_budget(request) {
            tracing::warn!(
                request_model = %request.model,
                normalized_model = CLAUDE_LONG_CONTEXT_MODEL,
                conversation_tokens = conversation_token_count(request),
                "routing an oversized Claude SubAgent through the long-context subscription model"
            );
            request.model = CLAUDE_LONG_CONTEXT_MODEL.to_owned();
            return Ok(RouteDecision::Subscription);
        }
        tracing::warn!(
            request_model = %request.model,
            normalized_model = model,
            "routing a Claude SubAgent tool request through the small fast model"
        );
        request.model = model.to_owned();
        return Ok(RouteDecision::Subscription);
    }

    apply_disabled_model_policy(request, origin.is_subagent)?;

    let explicit_native_claude = has_model_override
        && !origin.model_is_inherited
        && normalize_claude_model_to_haiku(&request.model).is_some();
    if origin.is_subagent && !explicit_native_claude && !supports_model(&request.model) {
        bail!(
            "SubAgent model `{}` does not have a recoverable configured route and must not be launched",
            request.model
        );
    }

    if supports_model(&request.model) {
        return Ok(RouteDecision::Provider);
    }
    if is_declared_provider_model(&request.model) {
        bail!(
            "configured provider model `{}` does not have an active route",
            request.model
        );
    }
    Ok(RouteDecision::Subscription)
}

fn conversation_token_count(request: &MessagesRequest) -> usize {
    serialized_len(&request.messages).div_ceil(4)
}

fn conversation_exceeds_haiku_budget(request: &MessagesRequest) -> bool {
    conversation_token_count(request) >= HAIKU_CONVERSATION_TOKEN_BUDGET
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RouteDecision {
    Provider,
    Subscription,
}

/// Identifies whether the SubAgent selected its model itself or inherited it.
#[derive(Clone, Copy)]
pub(super) struct RouteOrigin {
    is_subagent: bool,
    intent_matched: bool,
    model_is_inherited: bool,
}

impl RouteOrigin {
    pub(super) const fn new(
        is_subagent: bool,
        intent_matched: bool,
        model_is_inherited: bool,
    ) -> Self {
        Self {
            is_subagent,
            intent_matched,
            model_is_inherited,
        }
    }
}

fn apply_disabled_model_policy(request: &MessagesRequest, is_subagent: bool) -> Result<()> {
    if !request.disabled_subagent_models.contains(&request.model) {
        return Ok(());
    }
    if !is_subagent {
        return Ok(());
    }
    bail!(
        "SubAgent model `{}` is disabled by the active Claudex policy and must not be launched",
        request.model
    )
}

#[cfg(test)]
include!("request_routing_tests.rs");
