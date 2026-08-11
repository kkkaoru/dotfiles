use std::time::SystemTime;

use super::super::{
    Bridge, provider_auth, provider_auth_cooldown,
    request_routing::RouteDecision,
    segment::contains_empty_acp_billing_marker,
    stream::usage_limit::{
        contains_provider_quota_exhausted_marker, contains_rate_limit_marker,
        contains_usage_limit_marker,
    },
};
use super::UsageLimitFailover;

pub(super) fn push_model_auth_scopes(
    bridge: &Bridge,
    model: Option<&str>,
    scopes: &mut Vec<String>,
) {
    let Some(model) = model else {
        return;
    };
    scopes.push(model.to_owned());
    let Some(provider) = bridge.app.model_provider_for_model(model) else {
        return;
    };
    scopes.push(provider);
}

pub(super) const PREFERRED_SUBAGENT_FAILOVER: &[&str] = &["qwen3.8-max-preview"];

pub(super) fn ordered_subagent_failover_candidates(bridge: &Bridge) -> Vec<String> {
    let mut candidates = Vec::new();
    for model in PREFERRED_SUBAGENT_FAILOVER {
        candidates.push((*model).to_owned());
    }
    for worker in bridge.model_catalog.worker_routes() {
        candidates.push(worker.model.clone());
    }
    for model in bridge.app.models() {
        candidates.push(model);
    }
    candidates.sort();
    candidates.dedup();
    let mut ordered = Vec::new();
    for model in PREFERRED_SUBAGENT_FAILOVER {
        if candidates.iter().any(|candidate| candidate == model) {
            ordered.push((*model).to_owned());
        }
    }
    for model in candidates {
        if !ordered.iter().any(|preferred| preferred == &model) {
            ordered.push(model);
        }
    }
    ordered
}

pub(super) fn is_scoped_provider_exhaustion(message: &str) -> bool {
    contains_rate_limit_marker(message)
        || provider_auth::contains_auth_failure_marker(message)
        || contains_empty_acp_billing_marker(message)
        || contains_provider_quota_exhausted_marker(message)
}

pub(super) fn scoped_exhaustion_reason(
    message: &str,
    rate_limited: bool,
    quota_exhausted: bool,
) -> &'static str {
    if rate_limited {
        return "rate-limit";
    }
    if quota_exhausted {
        return "quota";
    }
    if contains_empty_acp_billing_marker(message) {
        return "empty-acp-billing";
    }
    "auth"
}

pub(super) fn record_scoped_exhaustion(
    cache_path: Option<&std::path::Path>,
    scope: &str,
    message: &str,
    reason: &'static str,
    use_rate_limit_record: bool,
) {
    let recorded = if use_rate_limit_record {
        provider_auth_cooldown::record_rate_limit_at(cache_path, scope, message, SystemTime::now())
    } else {
        provider_auth_cooldown::record_at(cache_path, scope, message, SystemTime::now())
    };
    let Some(path) = recorded else {
        return;
    };
    tracing::warn!(
        path = %path.display(),
        auth_scope = %scope,
        reason,
        error = %message,
        "recorded provider exhaustion cooldown; routing away from that provider"
    );
}

pub fn is_usage_limit_exceeded(error: &anyhow::Error) -> bool {
    should_failover_provider_error(error)
}

pub fn should_failover_provider_error(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    contains_usage_limit_marker(&message)
        || provider_auth::contains_auth_failure_marker(&message)
        || contains_empty_acp_billing_marker(&message)
}

/// Streaming turns can only continue on a sibling Provider.
/// Subscription failover cannot attach to an already-open SubAgent SSE stream,
/// which is why the old empty-ACP path returned the error to Claude Code.
pub fn streaming_provider_retry(
    failover: Option<UsageLimitFailover>,
) -> Option<UsageLimitFailover> {
    failover.filter(|candidate| candidate.route == RouteDecision::Provider)
}
