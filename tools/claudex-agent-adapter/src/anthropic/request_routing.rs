use super::agent_route_validation::BlockedSubagentError;
use super::{MessagesRequest, content::serialized_len};

mod models;
pub(crate) use models::{normalize_claude_model_to_haiku, official_claude_haiku_model};

mod resolve;
pub(super) use resolve::resolve_request_model_with_origin;

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
) -> anyhow::Result<RouteDecision> {
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

fn apply_disabled_model_policy(
    request: &MessagesRequest,
    is_subagent: bool,
) -> std::result::Result<(), BlockedSubagentError> {
    if !crate::subagent_policy::model_is_disabled(&request.disabled_subagent_models, &request.model)
    {
        return Ok(());
    }
    if !is_subagent {
        return Ok(());
    }
    Err(BlockedSubagentError::policy_disabled(&request.model))
}

#[cfg(test)]
include!("request_routing_tests.rs");
