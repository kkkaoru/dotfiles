use std::{future::Future, sync::Arc, time::Duration};

use anyhow::Result;
use tokio::time::{Instant, sleep};

use super::super::model_concurrency::{ModelPermit, Ticket, is_concurrency_admission_timeout};
use super::super::{Bridge, MessagesRequest, request_routing::RouteDecision};
use super::{
    ACTIVITY_KEEPALIVE_INTERVAL, INITIAL_ACTIVITY_DELAY, SegmentBuilder, StreamSender,
    send_stream_error,
};

pub(super) struct PreparedStream {
    pub(super) request: MessagesRequest,
    pub(super) input_tokens: u64,
    pub(super) effort: Option<String>,
    pub(super) concurrency_ticket: Option<Ticket>,
    pub(super) is_subagent: bool,
    pub(super) run_in_background: bool,
    pub(super) sender: StreamSender,
}

pub(super) fn subagent_start_status(
    is_subagent: bool,
    model: &str,
    effort: Option<&str>,
) -> Option<String> {
    is_subagent.then(|| {
        format!(
            "SubAgent starting: {model} (effort={}); preparing provider session\u{2026}",
            effort.unwrap_or("configured")
        )
    })
}

pub(super) async fn prepare_with_activity<F, T>(
    prepare: F,
    input_tokens: u64,
    sender: &StreamSender,
    initial_status: Option<&str>,
    first_delay: Duration,
    interval: Duration,
    is_subagent: bool,
) -> (Result<Option<T>>, SegmentBuilder)
where
    F: Future<Output = Result<T>>,
{
    let mut builder = SegmentBuilder::new(input_tokens).with_subagent(is_subagent);
    if let Some(status) = initial_status
        && let Err(error) = builder.subagent_start_status(status, Some(sender)).await
    {
        return (Err(error), builder);
    }
    let mut deadline = Box::pin(sleep(first_delay));
    tokio::pin!(prepare);
    loop {
        let result = tokio::select! {
            biased;
            () = sender.closed() => return (Ok(None), builder),
            result = &mut prepare => return (result.map(Some), builder),
            () = &mut deadline => builder.activity_keepalive(Some(sender)).await,
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
        } = prepared;
        self.reticket_saturated_subagent(
            &mut request,
            &mut effort,
            &mut concurrency_ticket,
            is_subagent,
        );
        let _active_subagent =
            is_subagent.then(|| self.active_subagent_models.acquire(&request.model));
        let start_status = subagent_start_status(is_subagent, &request.model, effort.as_deref());
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
            INITIAL_ACTIVITY_DELAY,
            ACTIVITY_KEEPALIVE_INTERVAL,
            is_subagent,
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
