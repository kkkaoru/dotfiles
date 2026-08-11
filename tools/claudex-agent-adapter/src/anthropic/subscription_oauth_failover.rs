use std::sync::Arc;

use anyhow::Result;
use axum::{body::Body, http::Response};

use super::{is_subscription_auth_failure, push_unique};
use super::super::{
    Bridge, MessagesRequest, request_routing::RouteDecision, token_count,
    usage_limit_failover::UsageLimitFailover,
};

impl Bridge {
    pub(in crate::anthropic) fn subscription_auth_failover_for(&self) -> Option<UsageLimitFailover> {
        let mut candidates = Vec::new();
        for worker in self.model_catalog.worker_routes() {
            candidates.push(worker.model.clone());
        }
        for model in self.app.models() {
            push_unique(&mut candidates, model);
        }
        candidates
            .into_iter()
            .find_map(|model| self.provider_failover_candidate(model))
    }

    fn provider_failover_candidate(&self, model: String) -> Option<UsageLimitFailover> {
        if self.subagent_provider_is_exhausted(&model) {
            return None;
        }
        self.app.backend_kind_for_model(&model)?;
        let effort = self
            .model_catalog
            .worker_effort_for_model(&model)
            .map(str::to_owned)
            .or_else(|| self.app.launch_scoped_effort(&model));
        Some(UsageLimitFailover {
            effort,
            model,
            route: RouteDecision::Provider,
        })
    }

    pub(in crate::anthropic) async fn subscription_messages_with_auth_failover(
        self: &Arc<Self>,
        request: MessagesRequest,
        effort: Option<String>,
        is_subagent: bool,
        tools_were_provided: bool,
    ) -> Result<Response<Body>> {
        let can_failover = !request.stream;
        let failover_request = can_failover.then(|| request.clone());
        match self
            .subscription_messages(request, effort.clone(), is_subagent, tools_were_provided)
            .await
        {
            Ok(response) => Ok(response),
            Err(error) if can_failover && is_subscription_auth_failure(&error) => {
                self.retry_after_subscription_auth_failure(
                    error,
                    failover_request,
                    effort,
                    is_subagent,
                )
                .await
            }
            Err(error) => Err(error),
        }
    }

    async fn retry_after_subscription_auth_failure(
        self: &Arc<Self>,
        error: anyhow::Error,
        failover_request: Option<MessagesRequest>,
        effort: Option<String>,
        is_subagent: bool,
    ) -> Result<Response<Body>> {
        self.note_provider_exhaustion(&error, None);
        let Some(mut retry) = failover_request else {
            return Err(error);
        };
        let Some(failover) = self.subscription_auth_failover_for() else {
            return Err(error);
        };
        tracing::warn!(
            failover_model = %failover.model,
            "failing over outer non-stream turn after Claude subscription OAuth failure"
        );
        retry.model = failover.model;
        let failover_effort = failover.effort.or(effort);
        let input_tokens = u64::try_from(token_count(&retry)).unwrap_or(u64::MAX);
        self.provider_messages(retry, input_tokens, failover_effort, is_subagent, false)
            .await
    }
}
