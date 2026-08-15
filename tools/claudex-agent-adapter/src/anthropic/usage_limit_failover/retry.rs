use std::sync::Arc;

use anyhow::Result;
use axum::{body::Body, http::Response};

use super::super::{Bridge, MessagesRequest, token_count};
use super::support::should_failover_provider_error;

impl Bridge {
    pub(in crate::anthropic) async fn provider_messages_with_usage_limit_failover(
        self: &Arc<Self>,
        request: MessagesRequest,
        effort: Option<String>,
        is_subagent: bool,
        _tools_were_provided: bool,
        run_in_background: bool,
    ) -> Result<Response<Body>> {
        let can_failover = !is_subagent && !request.stream;
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
                self.note_provider_exhaustion(&error, Some(&exhausted_model));
                Err(error.context(format!(
                    "requested model `{exhausted_model}` failed; failover is disabled"
                )))
            }
            Err(error) => Err(error),
        }
    }
}
