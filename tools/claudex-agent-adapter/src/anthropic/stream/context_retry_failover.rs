use anyhow::Result;

use super::super::super::request_routing::RouteDecision;
use super::super::super::usage_limit_failover::UsageLimitFailover;
use super::super::super::{ActiveTurn, Bridge, ContextRetry};
use super::UsageLimitOutcome;

impl Bridge {
    pub(in crate::anthropic::stream) async fn failover_usage_limit_turn(
        &self,
        mut turn: ActiveTurn,
        error: anyhow::Error,
    ) -> Result<UsageLimitOutcome> {
        let exhausted_model = turn.session.model.clone();
        self.note_provider_exhaustion(&error, Some(&exhausted_model));
        let error_text = error.to_string();
        let Some(retry) = turn.retry.take() else {
            self.remove_session(&turn.session).await;
            return Err(error);
        };
        if crate::anthropic::request_identity::authoritative_is_subagent(&retry.request)
            != Some(true)
        {
            self.remove_session(&turn.session).await;
            return Err(error.context(format!(
                "requested model `{exhausted_model}` failed; failover is disabled"
            )));
        }
        let Some(failover) = self.subagent_provider_failover_for(&exhausted_model) else {
            self.remove_session(&turn.session).await;
            return Err(error);
        };
        match failover.route {
            RouteDecision::Provider => {
                self.continue_on_sibling_provider(
                    turn,
                    retry,
                    failover,
                    exhausted_model,
                    &error_text,
                )
                .await
            }
            RouteDecision::Subscription => {
                self.failover_completed_to_subscription(
                    turn,
                    retry,
                    failover,
                    exhausted_model,
                    &error_text,
                )
                .await
            }
        }
    }

    async fn continue_on_sibling_provider(
        &self,
        turn: ActiveTurn,
        mut retry: ContextRetry,
        failover: UsageLimitFailover,
        exhausted_model: String,
        error_text: &str,
    ) -> Result<UsageLimitOutcome> {
        tracing::warn!(
            error = %error_text,
            exhausted_model = %exhausted_model,
            failover_model = %failover.model,
            "retrying completed turn on a sibling provider after provider exhaustion"
        );
        retry.request.model = failover.model;
        if let Some(effort) = failover.effort {
            retry.effort = Some(effort);
        }
        let input_tokens = turn.input_tokens;
        let previous = std::sync::Arc::clone(&turn.session);
        drop(turn);
        Ok(UsageLimitOutcome::Continue(Box::new(
            self.retry_after_context_window(retry, &previous, input_tokens)
                .await?,
        )))
    }

    async fn failover_completed_to_subscription(
        &self,
        turn: ActiveTurn,
        retry: ContextRetry,
        failover: UsageLimitFailover,
        exhausted_model: String,
        error_text: &str,
    ) -> Result<UsageLimitOutcome> {
        tracing::warn!(
            error = %error_text,
            exhausted_model = %exhausted_model,
            failover_model = %failover.model,
            "failing over completed turn to subscription after usageLimitExceeded"
        );
        let mut request = retry.request;
        request.model = failover.model;
        let effort = failover.effort.or(retry.effort);
        let tools_were_provided = !request.tools.is_empty();
        self.remove_session(&turn.session).await;
        Ok(UsageLimitOutcome::Response(Box::new(
            self.subscription_messages(request, effort, false, tools_were_provided)
                .await?,
        )))
    }
}
