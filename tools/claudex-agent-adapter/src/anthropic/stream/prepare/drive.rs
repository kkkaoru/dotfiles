use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use super::super::{
    ACTIVITY_KEEPALIVE_INTERVAL, INITIAL_ACTIVITY_DELAY, SUBAGENT_INITIAL_ACTIVITY_DELAY,
    send_stream_error,
};
use super::{
    PrepareActivityOptions, PreparedStream, SegmentBuilder, StreamSender, prepare_with_activity,
    subagent_start_status,
};
use crate::anthropic::{
    Bridge, MessagesRequest,
    model_concurrency::{ModelPermit, Ticket, is_concurrency_admission_timeout},
    request_routing::RouteDecision,
};

pub(in crate::anthropic::stream) fn prepare_first_activity_delay(
    is_subagent: bool,
    primed_thinking: bool,
) -> Duration {
    if primed_thinking {
        // `message_start` is already queued. Keep the first preparation tick
        // prompt, but it must remain content-free until real provider output.
        Duration::ZERO
    } else if is_subagent {
        SUBAGENT_INITIAL_ACTIVITY_DELAY
    } else {
        INITIAL_ACTIVITY_DELAY
    }
}

impl Bridge {
    pub(in crate::anthropic) async fn drive_prepared_subagent_stream(
        self: Arc<Self>,
        prepared: PreparedStream,
    ) {
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
        let _active_subagent = self.track_active_subagent(is_subagent, &request);
        let paint_command_code_progress =
            is_subagent && crate::command_code_acp::is_command_code_model(&request.model);
        let start_status = (!primed_thinking)
            .then(|| subagent_start_status(is_subagent, &request.model, effort.as_deref()))
            .flatten();
        let first_delay = prepare_first_activity_delay(is_subagent, primed_thinking);
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
            PrepareActivityOptions {
                input_tokens,
                sender: &sender,
                initial_status: start_status.as_deref(),
                first_delay,
                interval,
                is_subagent,
                paint_command_code_progress,
                primed_thinking,
            },
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

    pub(in crate::anthropic::stream::prepare) fn reticket_saturated_subagent(
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

    pub(in crate::anthropic::stream::prepare) async fn acquire_prepared_permit(
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
                self.retry_after_concurrency_timeout(request, effort, error)
                    .await
            }
            Err(error) => Err(error),
        }
    }

    pub(in crate::anthropic::stream::prepare) async fn retry_after_concurrency_timeout(
        &self,
        request: &mut MessagesRequest,
        effort: &mut Option<String>,
        error: anyhow::Error,
    ) -> Result<Option<ModelPermit>> {
        let Some(retry_ticket) = self.reticket_after_concurrency_timeout(request, effort) else {
            return Err(error);
        };
        match retry_ticket {
            Some(ticket) => Ok(Some(ticket.acquire_for(false).await?)),
            None => Ok(None),
        }
    }

    pub(in crate::anthropic::stream::prepare) async fn fail_prepared_stream(
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
