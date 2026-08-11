mod select;
mod support;
mod note;
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

pub(super) use support::streaming_provider_retry;
pub(crate) use support::{is_usage_limit_exceeded, should_failover_provider_error};

use std::{sync::Arc, time::SystemTime};

use anyhow::Result;
use axum::{body::Body, http::Response};

use super::{Bridge, MessagesRequest, request_routing::RouteDecision, token_count};

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
        effort: &mut Option<String>,
        is_subagent: bool,
    ) -> RouteDecision {
        // SubAgents keep their routed provider; the orchestrator re-routes from
        // capacity context instead of burning Claude subscription quota.
        if is_subagent || route != RouteDecision::Provider {
            return route;
        }
        let reason = if self.codex_usage_limit_is_active(&request.model) {
            Some("usage-limit")
        } else if self.provider_auth_is_cooling_down(&request.model) {
            Some("auth")
        } else {
            None
        };
        let Some(reason) = reason else {
            return route;
        };
        let Some(failover) = self.usage_limit_failover_for(&request.model) else {
            tracing::warn!(
                model = %request.model,
                reason,
                "provider is cooling down but no failover target is configured"
            );
            return route;
        };
        tracing::warn!(
            exhausted_model = %request.model,
            failover_model = %failover.model,
            failover_route = ?failover.route,
            reason,
            "preflight failover away from exhausted provider"
        );
        request.model = failover.model;
        if let Some(failover_effort) = failover.effort {
            *effort = Some(failover_effort);
        }
        failover.route
    }

    pub(super) async fn provider_messages_with_usage_limit_failover(
        self: &Arc<Self>,
        request: MessagesRequest,
        effort: Option<String>,
        is_subagent: bool,
        tools_were_provided: bool,
        run_in_background: bool,
    ) -> Result<Response<Body>> {
        let can_failover = !is_subagent && !request.stream;
        let failover_request = can_failover.then(|| request.clone());
        let exhausted_model = request.model.clone();
        let input_tokens = u64::try_from(token_count(&request)).unwrap_or(u64::MAX);
        match self
            .provider_messages(
                request,
                input_tokens,
                effort.clone(),
                is_subagent,
                run_in_background,
            )
            .await
        {
            Ok(response) => Ok(response),
            Err(error) if can_failover && should_failover_provider_error(&error) => {
                self.retry_after_provider_exhaustion(
                    error,
                    failover_request,
                    &exhausted_model,
                    effort,
                    tools_were_provided,
                )
                .await
            }
            Err(error) => Err(error),
        }
    }

    async fn retry_after_provider_exhaustion(
        self: &Arc<Self>,
        error: anyhow::Error,
        failover_request: Option<MessagesRequest>,
        exhausted_model: &str,
        effort: Option<String>,
        tools_were_provided: bool,
    ) -> Result<Response<Body>> {
        self.note_provider_exhaustion(&error, Some(exhausted_model));
        let Some(mut retry) = failover_request else {
            return Err(error);
        };
        let Some(failover) = self.usage_limit_failover_for(exhausted_model) else {
            return Err(error);
        };
        tracing::warn!(
            target: "claudex.provider",
            log_event = "provider_failover",
            exhausted_model = %exhausted_model,
            failover_model = %failover.model,
            failover_route = ?failover.route,
            "failing over outer non-stream turn after provider exhaustion"
        );
        retry.model = failover.model;
        let failover_effort = failover.effort.or(effort);
        self.dispatch_failover_messages(retry, failover.route, failover_effort, tools_were_provided)
            .await
    }

    async fn dispatch_failover_messages(
        self: &Arc<Self>,
        retry: MessagesRequest,
        route: RouteDecision,
        failover_effort: Option<String>,
        tools_were_provided: bool,
    ) -> Result<Response<Body>> {
        if route == RouteDecision::Subscription {
            return self
                .subscription_messages(retry, failover_effort, false, tools_were_provided)
                .await;
        }
        let input_tokens = u64::try_from(token_count(&retry)).unwrap_or(u64::MAX);
        self.provider_messages(retry, input_tokens, failover_effort, false, false)
            .await
    }
}
