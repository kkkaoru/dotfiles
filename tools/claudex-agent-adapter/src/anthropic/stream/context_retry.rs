use anyhow::Result;
use axum::{body::Body, http::Response};

use super::super::{ActiveTurn, Bridge, content::anthropic_response};

impl Bridge {
    pub(in crate::anthropic) async fn retry_after_provider_failure(
        &self,
        mut turn: ActiveTurn,
        error: anyhow::Error,
    ) -> Result<ActiveTurn> {
        if !super::is_provider_stream_closed(&error) || self.app.model_is_alive(&turn.session.model)
        {
            self.remove_session(&turn.session).await;
            return Err(error);
        }
        let Some(retry) = turn.retry.take() else {
            self.remove_session(&turn.session).await;
            return Err(error);
        };
        let model = turn.session.model.clone();
        let input_tokens = turn.input_tokens;
        let previous = std::sync::Arc::clone(&turn.session);
        tracing::warn!(
            model = %model,
            "retrying a provider failure after the routed ACP process was recycled"
        );
        self.retry_after_context_window(retry, &previous, input_tokens)
            .await
    }

    pub(in crate::anthropic) async fn context_retry_or_error(
        &self,
        turn: &mut ActiveTurn,
        error: anyhow::Error,
    ) -> Result<super::super::ContextRetry> {
        let retry = super::super::session::is_context_window_exceeded(&error)
            .then(|| turn.retry.take())
            .flatten();
        match retry {
            Some(retry) => Ok(retry),
            None => {
                self.remove_session(&turn.session).await;
                Err(error)
            }
        }
    }

    // This provider retry loop deliberately transfers one ActiveTurn between attempts.
    #[allow(clippy::excessive_nesting)]
    pub(in crate::anthropic) async fn non_streaming_response(
        &self,
        mut turn: ActiveTurn,
    ) -> Result<Response<Body>> {
        loop {
            let segment = self
                .wait_for_segment(
                    &turn.session,
                    &turn.events,
                    turn.input_tokens,
                    &turn.extras,
                    &turn.routing_system,
                    None,
                )
                .await;
            match segment {
                Ok(segment) => {
                    super::commit_transcript(&turn.session, turn.extras, &segment).await;
                    return Ok(anthropic_response(segment, &turn.response_model));
                }
                Err(error) => {
                    let error_text = error.to_string();
                    let retry = self.context_retry_or_error(&mut turn, error).await?;
                    tracing::warn!(
                        error = %error_text,
                        thread_id = %turn.session.thread_id,
                        "retrying completed turn on a fresh thread after context window exceeded"
                    );
                    turn = self
                        .retry_after_context_window(retry, &turn.session, turn.input_tokens)
                        .await?;
                }
            }
        }
    }
}
