use std::future::Future;

use agent_client_protocol::{self as acp, Agent as _};
use anyhow::anyhow;

use super::{
    ActiveTurns, CancelCtx, CancelRequest, CancellationSettlementTimeout, Settlement,
    SettlementPolicy, SetupCancellationSettlementTimeout, continue_after_cancel_request,
    dispatch_turn_terminal, settle_cancelled_prompt,
};
use crate::app_server::events::ThreadEventDispatcher;
use crate::grok_acp::updates;

pub(in crate::grok_acp::turns) fn finish_setup_cancellation(
    session_id: &str,
    permit: tokio::sync::OwnedSemaphorePermit,
    cancellation: CancelRequest,
    events: &ThreadEventDispatcher,
    active_turns: &ActiveTurns,
) {
    // ACP session/cancel only applies after a session/prompt request is in flight.
    drop(permit);
    active_turns.borrow_mut().remove(session_id);
    let _ = cancellation.response.send(Ok(()));
    dispatch_turn_terminal(events, session_id, "cancelled");
}

pub(in crate::grok_acp::turns) async fn cancel_setup<F, T>(
    ctx: CancelCtx<'_>,
    active_turns: &ActiveTurns,
    setup: F,
) where
    F: Future<Output = T>,
{
    let policy = SettlementPolicy::default();
    match policy.settle(setup).await {
        Settlement::Settled(_) => {
            finish_setup_cancellation(
                ctx.session_id,
                ctx.permit,
                ctx.cancellation,
                ctx.events,
                active_turns,
            );
        }
        Settlement::TimedOut => {
            let error = SetupCancellationSettlementTimeout {
                provider: ctx.provider,
                session_id: ctx.session_id.to_owned(),
                timeout: policy.timeout,
            };
            let message = error.to_string();
            ctx.invalidated_sessions
                .borrow_mut()
                .insert(ctx.session_id.to_owned());
            drop(ctx.permit);
            active_turns.borrow_mut().remove(ctx.session_id);
            let _ = ctx.cancellation.response.send(Err(anyhow!(error)));
            updates::dispatch_error(ctx.events, ctx.session_id, message);
        }
    }
}

pub(in crate::grok_acp::turns) async fn cancel_prompt<F>(
    ctx: CancelCtx<'_>,
    connection: &acp::ClientSideConnection,
    prompt: F,
) where
    F: Future<Output = acp::Result<acp::PromptResponse>>,
{
    let policy = SettlementPolicy::default();
    let cancel = connection.cancel(acp::CancelNotification::new(ctx.session_id.to_owned()));
    let Some(ctx) = continue_after_cancel_request(ctx, policy, policy.settle(cancel).await) else {
        return;
    };
    let response = match policy.settle(prompt).await {
        Settlement::Settled(response) => response,
        Settlement::TimedOut => {
            let error = CancellationSettlementTimeout {
                provider: ctx.provider,
                session_id: ctx.session_id.to_owned(),
                timeout: policy.timeout,
            };
            let message = error.to_string();
            ctx.invalidated_sessions
                .borrow_mut()
                .insert(ctx.session_id.to_owned());
            drop(ctx.permit);
            let _ = ctx.cancellation.response.send(Err(anyhow!(error)));
            updates::dispatch_error(ctx.events, ctx.session_id, message);
            return;
        }
    };
    // ACP notification handlers are local tasks and can still have final updates
    // queued when the cancelled prompt response arrives.
    tokio::task::yield_now().await;
    settle_cancelled_prompt(ctx, response);
}
