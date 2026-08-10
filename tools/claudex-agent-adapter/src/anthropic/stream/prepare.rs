use std::{future::Future, sync::Arc, time::Duration};

use anyhow::Result;
use tokio::time::{Instant, sleep};

use axum::body::Bytes;
use serde_json::json;

use super::super::model_concurrency::{ModelPermit, Ticket, is_concurrency_admission_timeout};
use super::super::{Bridge, MessagesRequest, content::sse, request_routing::RouteDecision};
use super::{
    ACTIVITY_KEEPALIVE_INTERVAL, INITIAL_ACTIVITY_DELAY, SUBAGENT_INITIAL_ACTIVITY_DELAY,
    SegmentBuilder, StreamSender, message_start, send_stream_error,
};

pub(super) struct PreparedStream {
    pub(super) request: MessagesRequest,
    pub(super) input_tokens: u64,
    pub(super) effort: Option<String>,
    pub(super) concurrency_ticket: Option<Ticket>,
    pub(super) is_subagent: bool,
    pub(super) run_in_background: bool,
    pub(super) sender: StreamSender,
    pub(super) primed_thinking: bool,
}

pub(super) fn subagent_start_status(
    is_subagent: bool,
    model: &str,
    effort: Option<&str>,
) -> Option<String> {
    if !is_subagent || crate::command_code_acp::is_command_code_model(model) {
        return None;
    }
    let effort = effort.unwrap_or("configured");
    Some(format!(
        "SubAgent starting: {model} (effort={effort}); preparing provider session\u{2026}"
    ))
}

pub(super) fn prime_subagent_sse(
    sender: &StreamSender,
    model: &str,
    input_tokens: u64,
    is_subagent: bool,
    effort: Option<&str>,
) -> bool {
    sender
        .try_send(Ok(Bytes::from(message_start(model, input_tokens))))
        .expect("new streaming response channel has capacity");
    // Claude Code drops SSE after message_start unless a thinking block lands in
    // the same first flush. Main turns also sit blank during ACP session/new +
    // permit acquire; paint immediately for every provider hop through claudex.
    let first_delta = if is_subagent {
        if crate::command_code_acp::is_command_code_model(model) {
            "\u{200b}".to_owned()
        } else {
            subagent_start_status(true, model, effort)
                .unwrap_or_else(|| "SubAgent starting\u{2026}".to_owned())
        }
    } else {
        "Preparing provider session\u{2026}".to_owned()
    };
    for frame in subagent_thinking_prime_sse(&first_delta) {
        let _ = sender.try_send(Ok(Bytes::from(frame)));
    }
    true
}

fn subagent_thinking_prime_sse(first_delta: &str) -> [String; 2] {
    [
        sse(
            "content_block_start",
            json!({
                "type":"content_block_start",
                "index":0,
                "content_block":{"type":"thinking","thinking":"","signature":""}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type":"content_block_delta",
                "index":0,
                "delta":{"type":"thinking_delta","thinking":first_delta}
            }),
        ),
    ]
}

pub(super) async fn prepare_with_activity<F, T>(
    prepare: F,
    input_tokens: u64,
    sender: &StreamSender,
    initial_status: Option<&str>,
    first_delay: Duration,
    interval: Duration,
    is_subagent: bool,
    paint_command_code_progress: bool,
    primed_thinking: bool,
) -> (Result<Option<T>>, SegmentBuilder)
where
    F: Future<Output = Result<T>>,
{
    let mut builder = SegmentBuilder::new(input_tokens)
        .with_subagent(is_subagent)
        .with_command_code_progress(paint_command_code_progress);
    let mut sse_open = true;
    if primed_thinking {
        builder = builder.with_primed_thinking();
    } else if let Some(status) = initial_status
        && let Err(error) = builder.subagent_start_status(status, Some(sender)).await
    {
        return (Err(error), builder);
    }
    let mut deadline = Box::pin(sleep(first_delay));
    tokio::pin!(prepare);
    loop {
        let result = tokio::select! {
            biased;
            () = sender.closed(), if sse_open => {
                if !is_subagent {
                    return (Ok(None), builder);
                }
                // Claude Code often drops SubAgent SSE immediately after
                // message_start. Keep preparing so ACP/cmd is not torn down.
                sse_open = false;
                continue;
            }
            result = &mut prepare => return (result.map(Some), builder),
            () = &mut deadline => builder.activity_keepalive(sse_open.then_some(sender)).await,
        };
        if let Err(error) = result {
            return (Err(error), builder);
        }
        deadline.as_mut().reset(Instant::now() + interval);
    }
}

impl Bridge {
    pub(super) async fn drive_prepared_subagent_stream(self: Arc<Self>, prepared: PreparedStream) {
        let PreparedStream {
            mut request,
            input_tokens,
            mut effort,
            mut concurrency_ticket,
            is_subagent,
            run_in_background,
            sender,
            primed_thinking,
        } = prepared;
        self.reticket_saturated_subagent(
            &mut request,
            &mut effort,
            &mut concurrency_ticket,
            is_subagent,
        );
        let _active_subagent =
            is_subagent.then(|| self.active_subagent_models.acquire(&request.model));
        let paint_command_code_progress =
            is_subagent && crate::command_code_acp::is_command_code_model(&request.model);
        let start_status = (!primed_thinking)
            .then(|| subagent_start_status(is_subagent, &request.model, effort.as_deref()))
            .flatten();
        let first_delay = if is_subagent {
            SUBAGENT_INITIAL_ACTIVITY_DELAY
        } else {
            INITIAL_ACTIVITY_DELAY
        };
        let interval = ACTIVITY_KEEPALIVE_INTERVAL;
        let prepare = async {
            let permit = self
                .acquire_prepared_permit(&mut request, &mut effort, concurrency_ticket, is_subagent)
                .await?;
            let turn = self.prepare_turn(&request, input_tokens, effort).await?;
            Ok((turn, permit))
        };
        let (turn, mut builder) = prepare_with_activity(
            prepare,
            input_tokens,
            &sender,
            start_status.as_deref(),
            first_delay,
            interval,
            is_subagent,
            paint_command_code_progress,
            primed_thinking,
        )
        .await;
        match turn {
            Ok(Some((turn, permit))) => {
                self.drive_subagent_stream(
                    turn,
                    sender,
                    builder,
                    permit,
                    is_subagent,
                    run_in_background,
                )
                .await
            }
            Ok(None) => {}
            Err(error) => {
                self.fail_prepared_stream(&sender, &mut builder, error, &request.model)
                    .await;
            }
        }
    }

    fn reticket_saturated_subagent(
        &self,
        request: &mut MessagesRequest,
        effort: &mut Option<String>,
        concurrency_ticket: &mut Option<Ticket>,
        is_subagent: bool,
    ) {
        if !is_subagent {
            return;
        }
        let original_model = request.model.clone();
        let _ = self.apply_concurrency_preflight(request, RouteDecision::Provider, effort, true);
        if request.model != original_model {
            *concurrency_ticket = self.model_concurrency.ticket(
                &request.model,
                self.app.max_concurrency_for_model(&request.model),
            );
        }
    }

    async fn acquire_prepared_permit(
        &self,
        request: &mut MessagesRequest,
        effort: &mut Option<String>,
        concurrency_ticket: Option<Ticket>,
        is_subagent: bool,
    ) -> Result<Option<ModelPermit>> {
        let Some(ticket) = concurrency_ticket else {
            return Ok(None);
        };
        match ticket.acquire_for(!is_subagent).await {
            Ok(permit) => Ok(Some(permit)),
            Err(error) if is_subagent && is_concurrency_admission_timeout(&error) => {
                let Some(retry_ticket) = self.reticket_after_concurrency_timeout(request, effort)
                else {
                    return Err(error);
                };
                match retry_ticket {
                    Some(ticket) => Ok(Some(ticket.acquire_for(false).await?)),
                    None => Ok(None),
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn fail_prepared_stream(
        &self,
        sender: &StreamSender,
        builder: &mut SegmentBuilder,
        error: anyhow::Error,
        model: &str,
    ) {
        if !is_concurrency_admission_timeout(&error) {
            self.note_provider_exhaustion(&error, Some(model));
        }
        let _ = builder.close_open_blocks(Some(sender)).await;
        send_stream_error(sender, error).await;
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "prepare_tests.rs"]
mod tests;
