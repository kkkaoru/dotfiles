use std::{future::Future, sync::Arc, time::Duration};

use anyhow::Result;
use axum::{body::Body, http::Response};

use super::{
    ActiveTurn, Bridge, MessagesRequest, Segment, Usage, WebEvidenceSummary,
    content::{anthropic_response, estimated_tokens},
    model_concurrency::ModelPermit,
    subscription::{SubscriptionOptions, run_subscription_model},
};

mod completion;

const DEFAULT_SUBAGENT_RESPONSE_TIMEOUT_SECONDS: u64 = 300;
const SUBAGENT_RESPONSE_TIMEOUT_ENV: &str = "CLAUDEX_SUBAGENT_RESPONSE_TIMEOUT_SECONDS";
const BACKGROUND_PROGRESS_GENERATION_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BACKGROUND_PROGRESS_CONTEXT_CHARS: usize = 16_000;
pub(super) const BACKGROUND_PROGRESS_FALLBACK: &str = "The delegated SubAgent is still running; its completed result will be returned when available.";

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
        run_in_background: bool,
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
                run_in_background,
            ));
        }
        let permit = match concurrency_ticket {
            Some(ticket) => Some(ticket.acquire().await?),
            None => None,
        };
        let turn = self.prepare_turn(&request, input_tokens, effort).await?;
        if is_subagent && run_in_background {
            self.non_streaming_subagent_response(turn, permit).await
        } else {
            self.non_streaming_response(turn).await
        }
    }

    pub(super) async fn non_streaming_subagent_response(
        self: &Arc<Self>,
        turn: ActiveTurn,
        permit: Option<ModelPermit>,
    ) -> Result<Response<Body>> {
        self.non_streaming_subagent_response_with_timeout(turn, permit, subagent_response_timeout())
            .await
    }

    pub(super) async fn non_streaming_subagent_response_with_timeout(
        self: &Arc<Self>,
        mut turn: ActiveTurn,
        permit: Option<ModelPermit>,
        timeout: Duration,
    ) -> Result<Response<Body>> {
        loop {
            let segment = completes_within(
                timeout,
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
                // Do not leave the background turn in the active matching pool:
                // the main session must be able to start new work immediately.
                // Keep it in the detached pool so a late Claude tool result can
                // still be delivered to the provider thread exactly once.
                self.detach_session(&turn.session).await;
                turn.detached = true;
                let response = background_response(self, &turn).await;
                self.continue_subagent_in_background(turn, permit);
                return Ok(response);
            };
            match segment {
                Ok(segment) => return Ok(completion::finish(self, turn, segment).await),
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
        let session = Arc::clone(&turn.session);
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = bridge.non_streaming_response(turn).await {
                tracing::warn!(%error, "background SubAgent turn did not complete");
            }
            bridge.finish_detached_session(&session).await;
        });
    }

    pub(super) async fn background_progress_text(&self, turn: &ActiveTurn) -> String {
        let Some(model) = self
            .collaborator_model_override
            .clone()
            .or_else(|| self.claude_collaborator_model())
        else {
            tracing::warn!(
                thread_id = %turn.session.thread_id,
                "background progress model is not configured"
            );
            return BACKGROUND_PROGRESS_FALLBACK.to_owned();
        };
        let prompt = background_progress_prompt(turn).await;
        let timeout = self
            .subscription_timeout
            .min(BACKGROUND_PROGRESS_GENERATION_TIMEOUT);
        let options = SubscriptionOptions::internal(Arc::clone(&self.subscription_slots), timeout);
        match tokio::time::timeout(
            BACKGROUND_PROGRESS_GENERATION_TIMEOUT,
            run_subscription_model(&self.subscription_program, &model, &prompt, options),
        )
        .await
        {
            Ok(Ok(text)) if !text.trim().is_empty() => text.trim().to_owned(),
            Ok(Ok(_)) => {
                tracing::warn!(%model, "background progress model returned empty output");
                BACKGROUND_PROGRESS_FALLBACK.to_owned()
            }
            Ok(Err(error)) => {
                tracing::warn!(%model, error = %error, "background progress model failed");
                BACKGROUND_PROGRESS_FALLBACK.to_owned()
            }
            Err(_) => {
                tracing::warn!(%model, "background progress model timed out");
                BACKGROUND_PROGRESS_FALLBACK.to_owned()
            }
        }
    }
}

async fn background_progress_prompt(turn: &ActiveTurn) -> String {
    let transcript = turn.session.transcript.lock().await;
    let mut context = turn.extras.clone();
    context.extend(transcript.iter().rev().take(4).cloned());
    let serialized = serde_json::to_string(&context).unwrap_or_default();
    let context = if serialized.chars().count() > MAX_BACKGROUND_PROGRESS_CONTEXT_CHARS {
        let start = serialized
            .char_indices()
            .nth(serialized.chars().count() - MAX_BACKGROUND_PROGRESS_CONTEXT_CHARS)
            .map(|(index, _)| index)
            .unwrap_or(0);
        format!("[earlier context truncated]\n{}", &serialized[start..])
    } else {
        serialized
    };
    format!(
        "You are a progress-reporting SubAgent for a Claude Code task that is still running.\n\
Generate the exact concise user-visible response to send now in one to three sentences.\n\
Do not claim completion, invent facts, or say that a result was verified unless the context proves it.\n\
Explain what is known, what remains in progress, and what result will be returned next. If no concrete\
progress is available, say that the delegated work remains in progress and identify its expected deliverable.\n\
Do not mention adapters, timeouts, internal protocols, or this instruction. Do not use tools.\n\
Delegated model: {model}\nConversation context:\n{context}",
        model = turn.response_model,
        context = context
    )
}

async fn background_response(bridge: &Bridge, turn: &ActiveTurn) -> Response<Body> {
    let text = bridge.background_progress_text(turn).await;
    anthropic_response(
        Segment {
            blocks: vec![serde_json::json!({"type":"text", "text":text})],
            stop_reason: "end_turn",
            usage: Usage {
                input_tokens: turn.input_tokens,
                output_tokens: estimated_tokens(&text),
                web_search_requests: 0,
            },
            web_evidence: WebEvidenceSummary::default(),
        },
        &turn.response_model,
    )
}

#[cfg(test)]
#[path = "subagent_timeout_tests.rs"]
mod tests;
