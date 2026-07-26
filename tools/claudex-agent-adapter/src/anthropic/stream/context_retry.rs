use anyhow::Result;
use axum::{body::Body, http::Response};

use super::super::{ActiveTurn, Bridge, content::anthropic_response};

impl Bridge {
    async fn context_retry_or_error(
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
