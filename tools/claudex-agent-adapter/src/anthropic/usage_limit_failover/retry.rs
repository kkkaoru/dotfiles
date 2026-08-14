use std::sync::Arc;

use anyhow::Result;
use axum::{body::Body, http::Response};

use super::super::{Bridge, MessagesRequest, request_routing::RouteDecision, token_count};
use super::support::should_failover_provider_error;

impl Bridge {
    pub(in crate::anthropic) async fn provider_messages_with_usage_limit_failover(
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::agent_backend::AgentBackend;

    #[tokio::test]
    async fn retry_without_a_preserved_outer_request_returns_the_original_error() {
        let cache_home = tempfile::tempdir().expect("failover cache home");
        let bridge = Arc::new(
            Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned())
                .with_usage_limit_cache_home(cache_home.path()),
        );
        let error = bridge
            .retry_after_provider_exhaustion(
                anyhow::anyhow!("provider exhausted"),
                None,
                "main",
                None,
                false,
            )
            .await
            .expect_err("missing preserved request must not attempt a failover");

        assert_eq!(error.to_string(), "provider exhausted");
    }

    #[tokio::test]
    async fn dispatches_a_provider_failover_with_recomputed_input_tokens() {
        let cache_home = tempfile::tempdir().expect("failover cache home");
        let bridge = Arc::new(
            Bridge::new_with_backend(AgentBackend::spawn_routes(&[]), "main".to_owned())
                .with_usage_limit_cache_home(cache_home.path()),
        );
        let request = MessagesRequest {
            model: "main".to_owned(),
            system: serde_json::Value::Null,
            messages: vec![serde_json::json!({
                "role": "user",
                "content": "provider dispatch coverage"
            })],
            tools: Vec::new(),
            stream: false,
            output_config: serde_json::Value::Null,
            metadata: serde_json::Value::Null,
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        };

        let result = bridge
            .dispatch_failover_messages(request, RouteDecision::Provider, None, false)
            .await;
        assert!(
            result.is_err(),
            "an empty backend must reject provider dispatch"
        );
    }
}
