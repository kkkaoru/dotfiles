use std::{future::Future, sync::Arc, time::Duration};

use anyhow::Result;
use axum::{body::Body, http::Response};

use super::{
    ActiveTurn, Bridge, MessagesRequest, Segment, Usage, content::anthropic_response,
    model_concurrency::ModelPermit,
};

const DEFAULT_SUBAGENT_RESPONSE_TIMEOUT_SECONDS: u64 = 60;
const SUBAGENT_RESPONSE_TIMEOUT_ENV: &str = "CLAUDEX_SUBAGENT_RESPONSE_TIMEOUT_SECONDS";
pub(super) const BACKGROUND_NOTICE: &str = "SubAgent is still processing in the background. Do not retry it immediately; continue the task and give the user a concise progress update.";

pub(super) fn subagent_response_timeout() -> Duration {
    subagent_response_timeout_from(|name| std::env::var(name).ok())
}

fn subagent_response_timeout_from(get: impl Fn(&str) -> Option<String>) -> Duration {
    Duration::from_secs(
        get(SUBAGENT_RESPONSE_TIMEOUT_ENV)
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .unwrap_or(DEFAULT_SUBAGENT_RESPONSE_TIMEOUT_SECONDS),
    )
}

pub(super) async fn completes_within<T>(
    timeout: Duration,
    future: impl Future<Output = T>,
) -> Option<T> {
    tokio::time::timeout(timeout, future).await.ok()
}

impl Bridge {
    pub(super) async fn provider_messages(
        self: &Arc<Self>,
        request: MessagesRequest,
        input_tokens: u64,
        effort: Option<String>,
        is_subagent: bool,
    ) -> Result<Response<Body>> {
        let concurrency_ticket = self.model_concurrency.ticket(
            &request.model,
            self.app.max_concurrency_for_model(&request.model),
        );
        // Open SSE before prepare_turn so Claude Code receives message_start and
        // keepalives while the provider session starts.
        if request.stream {
            return Ok(self.streaming_messages(
                request,
                input_tokens,
                effort,
                concurrency_ticket,
                is_subagent,
            ));
        }
        let permit = match concurrency_ticket {
            Some(ticket) => Some(ticket.acquire().await?),
            None => None,
        };
        let turn = self.prepare_turn(&request, input_tokens, effort).await?;
        if is_subagent {
            self.non_streaming_subagent_response(turn, permit).await
        } else {
            self.non_streaming_response(turn).await
        }
    }

    pub(super) async fn non_streaming_subagent_response(
        self: &Arc<Self>,
        mut turn: ActiveTurn,
        permit: Option<ModelPermit>,
    ) -> Result<Response<Body>> {
        loop {
            let segment = completes_within(
                subagent_response_timeout(),
                self.wait_for_segment(
                    &turn.session,
                    &turn.events,
                    turn.input_tokens,
                    &turn.extras,
                    &turn.routing_system,
                    None,
                ),
            )
            .await;
            let Some(segment) = segment else {
                let response = background_response(&turn);
                self.continue_subagent_in_background(turn, permit);
                return Ok(response);
            };
            match segment {
                Ok(segment) => {
                    super::stream::commit_transcript(&turn.session, turn.extras, &segment).await;
                    return Ok(anthropic_response(segment, &turn.response_model));
                }
                Err(error) => {
                    let error_text = error.to_string();
                    let retry = self.context_retry_or_error(&mut turn, error).await?;
                    tracing::warn!(
                        error = %error_text,
                        thread_id = %turn.session.thread_id,
                        "retrying completed SubAgent turn after context window exceeded"
                    );
                    turn = self
                        .retry_after_context_window(retry, &turn.session, turn.input_tokens)
                        .await?;
                }
            }
        }
    }

    pub(super) fn continue_subagent_in_background(
        self: &Arc<Self>,
        turn: ActiveTurn,
        permit: Option<ModelPermit>,
    ) {
        let bridge = Arc::clone(self);
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = bridge.non_streaming_response(turn).await {
                tracing::warn!(%error, "background SubAgent turn did not complete");
            }
        });
    }
}

fn background_response(turn: &ActiveTurn) -> Response<Body> {
    anthropic_response(
        Segment {
            blocks: vec![serde_json::json!({"type":"text", "text":BACKGROUND_NOTICE})],
            stop_reason: "end_turn",
            usage: Usage {
                input_tokens: turn.input_tokens,
                output_tokens: 0,
            },
        },
        &turn.response_model,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_validates_the_subagent_response_timeout() {
        assert_eq!(
            subagent_response_timeout_from(|_| None),
            Duration::from_secs(60)
        );
        assert_eq!(
            subagent_response_timeout_from(|_| Some("7".to_owned())),
            Duration::from_secs(7)
        );
        assert_eq!(
            subagent_response_timeout_from(|_| Some("0".to_owned())),
            Duration::from_secs(60)
        );
    }

    #[tokio::test]
    async fn distinguishes_completed_and_backgrounded_work() {
        assert_eq!(
            completes_within(Duration::from_secs(1), async { 7 }).await,
            Some(7)
        );
        assert_eq!(
            completes_within(Duration::ZERO, std::future::pending::<u8>()).await,
            None
        );
    }
}
