use anyhow::{Result, bail};

use super::{
    MessagesRequest, RouteDecision, RouteOrigin, apply_disabled_model_policy,
    conversation_exceeds_haiku_budget, conversation_token_count,
    models::{CLAUDE_LONG_CONTEXT_MODEL, normalize_claude_model_to_haiku},
};

/// Resolve a request while retaining whether a SubAgent model came from its parent.
/// Explicit child model selections are never rewritten.
pub(in crate::anthropic) fn resolve_request_model_with_origin(
    request: &mut MessagesRequest,
    main_model: &str,
    model_override: Option<String>,
    origin: RouteOrigin,
    supports_model: impl Fn(&str) -> bool,
    // True when the model matches any provider identity declared in config (enabled or not).
    is_declared_provider_model: impl Fn(&str) -> bool,
) -> Result<RouteDecision> {
    // Stream painting may still treat historical `cc_is_subagent` as a child so
    // Muse Spark keeps live Thinking. Model routing must not: a session_id-only
    // transport header means the outer main asked for this model.
    let is_subagent =
        super::super::request_identity::authoritative_is_subagent(request).unwrap_or(origin.is_subagent);
    if is_subagent && (!origin.intent_matched || model_override.is_none()) {
        // Warn only if the request model is missing entirely; if a model is provided,
        // direct routing from Claude Code to a SubAgent is normal (e.g., nested SubAgent
        // or direct child launch). Debug-log when metadata is incomplete but model is explicit.
        if request.model.is_empty() {
            tracing::warn!(
                intent_matched = origin.intent_matched,
                has_explicit_model = model_override.is_some(),
                "SubAgent routing request lacks a model; using request default"
            );
        } else if !origin.intent_matched && model_override.is_none() {
            tracing::debug!(
                request_model = %request.model,
                "SubAgent routing: no prior Agent/Task intent; using request model directly"
            );
        }
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
    if is_subagent
        && (!has_model_override || origin.model_is_inherited)
        && request.model == CLAUDE_LONG_CONTEXT_MODEL
    {
        return Ok(RouteDecision::Subscription);
    }
    if is_subagent
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

    apply_disabled_model_policy(request, is_subagent)?;

    let explicit_native_claude = has_model_override
        && !origin.model_is_inherited
        && normalize_claude_model_to_haiku(&request.model).is_some();
    if is_subagent && !explicit_native_claude && !supports_model(&request.model) {
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
