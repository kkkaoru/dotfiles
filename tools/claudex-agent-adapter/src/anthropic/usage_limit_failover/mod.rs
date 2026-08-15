mod note;
mod retry;
mod select;
mod support;
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

pub(super) use support::streaming_provider_retry;
pub(crate) use support::{is_usage_limit_exceeded, should_failover_provider_error};

use std::time::SystemTime;

use super::{Bridge, MessagesRequest, request_routing::RouteDecision};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct UsageLimitFailover {
    pub(super) model: String,
    pub(super) effort: Option<String>,
    pub(super) route: RouteDecision,
}

impl Bridge {
    pub(super) fn subagent_provider_is_exhausted(&self, model: &str) -> bool {
        self.subagent_model_is_exhausted(model, None)
    }

    /// Hard exhaustion only: auth/rate/quota cooldown, classic usage-limit, or an
    /// explicit exhausted/disabled routing snapshot. Low remaining is a selection
    /// heuristic and must not block or rewrite an explicit SubAgent launch.
    pub(super) fn subagent_model_is_exhausted(
        &self,
        model: &str,
        quota: Option<&serde_json::Value>,
    ) -> bool {
        self.provider_auth_is_cooling_down(model)
            || self.codex_usage_limit_is_active(model)
            || super::routing_quota::live_cache_marks_model_exhausted(
                self.usage_routing_cache_path().as_deref(),
                model,
                SystemTime::now(),
            )
            || quota.is_some_and(|summary| {
                super::routing_quota::summary_marks_model_exhausted(summary, model)
            })
    }

    pub(super) fn apply_usage_limit_preflight(
        &self,
        request: &mut MessagesRequest,
        route: RouteDecision,
        _effort: &mut Option<String>,
        is_subagent: bool,
    ) -> anyhow::Result<RouteDecision> {
        // SubAgents keep their routed provider. Outer turns also stay on the
        // requested model: silent failover to another backend is disabled.
        if is_subagent || route != RouteDecision::Provider {
            return Ok(route);
        }
        let reason = if self.codex_usage_limit_is_active(&request.model) {
            Some("usage-limit")
        } else if self.provider_auth_is_cooling_down(&request.model) {
            Some("auth")
        } else {
            None
        };
        let Some(reason) = reason else {
            return Ok(route);
        };
        anyhow::bail!(
            "requested model `{}` preflight failed ({reason}); failover is disabled",
            request.model
        );
    }
}
