use std::{sync::Arc, time::SystemTime};

use anyhow::Result;
use axum::{body::Body, http::Response};

use crate::agent_backend::BackendKind;

use super::{
    Bridge, MessagesRequest, request_routing::RouteDecision,
    stream::usage_limit::contains_usage_limit_marker, token_count, usage_limit_cooldown,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct UsageLimitFailover {
    pub(super) model: String,
    pub(super) effort: Option<String>,
    pub(super) route: RouteDecision,
}

impl Bridge {
    pub(super) fn note_usage_limit_failure(&self, error: &anyhow::Error) {
        let message = error.to_string();
        if !contains_usage_limit_marker(&message) {
            return;
        }
        if let Some(path) =
            usage_limit_cooldown::record_codex_app_server_limit(&message, SystemTime::now())
        {
            tracing::warn!(
                path = %path.display(),
                error = %message,
                "recorded Codex app-server usage-limit cooldown; routing away from that backend"
            );
        }
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
        if !self.model_uses_codex_app_server(&request.model) {
            return route;
        }
        if !usage_limit_cooldown::codex_app_server_is_cooling_down(SystemTime::now()) {
            return route;
        }
        let Some(failover) = self.usage_limit_failover_for(&request.model) else {
            tracing::warn!(
                model = %request.model,
                "Codex app-server is cooling down but no failover target is configured"
            );
            return route;
        };
        tracing::warn!(
            exhausted_model = %request.model,
            failover_model = %failover.model,
            failover_route = ?failover.route,
            "preflight failover away from Codex app-server usage-limit cooldown"
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
            Err(error) if can_failover && is_usage_limit_exceeded(&error) => {
                self.note_usage_limit_failure(&error);
                let Some(mut retry) = failover_request else {
                    return Err(error);
                };
                let Some(failover) = self.usage_limit_failover_for(&exhausted_model) else {
                    return Err(error);
                };
                tracing::warn!(
                    target: "claudex.provider",
                    log_event = "provider_failover",
                    exhausted_model = %exhausted_model,
                    failover_model = %failover.model,
                    failover_route = ?failover.route,
                    "failing over outer non-stream turn after usageLimitExceeded"
                );
                retry.model = failover.model;
                let failover_effort = failover.effort.or(effort);
                if failover.route == RouteDecision::Subscription {
                    self.subscription_messages(retry, failover_effort, false, tools_were_provided)
                        .await
                } else {
                    let input_tokens = u64::try_from(token_count(&retry)).unwrap_or(u64::MAX);
                    self.provider_messages(retry, input_tokens, failover_effort, false, false)
                        .await
                }
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn usage_limit_failover_for(
        &self,
        exhausted_model: &str,
    ) -> Option<UsageLimitFailover> {
        let _ = exhausted_model;
        // Always recover outer turns onto Claude subscription. Sibling ACP
        // providers (Grok, etc.) may also be empty, and Subscription was proven
        // healthy on this daemon while Codex app-server models were dark.
        Some(UsageLimitFailover {
            model: "claude-sonnet-5".to_owned(),
            effort: Some("high".to_owned()),
            route: RouteDecision::Subscription,
        })
    }

    pub(super) fn model_uses_codex_app_server(&self, model: &str) -> bool {
        match self.app.backend_kind_for_model(model) {
            Some(kind) => kind == BackendKind::CodexAppServer,
            None => matches!(&*self.app, crate::agent_backend::AgentBackend::Codex(_)),
        }
    }
}

pub(crate) fn is_usage_limit_exceeded(error: &anyhow::Error) -> bool {
    contains_usage_limit_marker(&error.to_string())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};
    use crate::anthropic::Bridge;
    use crate::anthropic::request_routing::RouteDecision;
    use crate::provider_config::ModelCatalog;

    #[test]
    fn prefers_configured_subscription_fallback_before_other_providers() {
        let backend = AgentBackend::spawn_routes(&[
            BackendRoute::new("fugu", BackendKind::CodexAppServer),
            BackendRoute::new("grok-4.5", BackendKind::GrokAcp),
        ]);
        let mut catalog = ModelCatalog::default();
        catalog
            .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
                "claudex-sonnet",
                "claude-sonnet-5",
                "high",
            )])
            .expect("install fallback");
        let bridge =
            Bridge::new_with_backend(backend, "fugu".to_owned()).with_model_catalog(catalog);
        let failover = bridge
            .usage_limit_failover_for("fugu")
            .expect("failover target");
        assert_eq!(failover.model, "claude-sonnet-5");
        assert_eq!(failover.route, RouteDecision::Subscription);
    }

    #[test]
    fn falls_back_to_configured_subscription_when_only_codex_remains() {
        let backend =
            AgentBackend::spawn_routes(&[BackendRoute::new("fugu", BackendKind::CodexAppServer)]);
        let mut catalog = ModelCatalog::default();
        catalog
            .set_auxiliary_worker_routes(vec![crate::provider_config::WorkerRoute::new(
                "claudex-sonnet",
                "claude-sonnet-5",
                "high",
            )])
            .expect("install fallback");
        let bridge =
            Bridge::new_with_backend(backend, "fugu".to_owned()).with_model_catalog(catalog);
        let failover = bridge
            .usage_limit_failover_for("fugu")
            .expect("subscription failover");
        assert_eq!(failover.model, "claude-sonnet-5");
        assert_eq!(failover.route, RouteDecision::Subscription);
    }
}
