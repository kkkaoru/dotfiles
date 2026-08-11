use agent_client_protocol as acp;
use anyhow::anyhow;

use super::{CancelCtx, Settlement, SettlementPolicy, dispatch_turn_terminal};
use crate::grok_acp::{connection::AcpProvider, updates};

pub(super) fn continue_after_cancel_request(
    ctx: CancelCtx<'_>,
    policy: SettlementPolicy,
    settlement: Settlement<acp::Result<()>>,
) -> Option<CancelCtx<'_>> {
    match settlement {
        Settlement::Settled(Ok(())) => Some(ctx),
        Settlement::Settled(Err(error)) => {
            let message = format!(
                "{} ACP session/cancel failed: {error:?}",
                ctx.provider.label()
            );
            fail_cancellation(ctx, message);
            None
        }
        Settlement::TimedOut => {
            let message = format!(
                "{} ACP session `{}` cancel request did not complete within {:?}",
                ctx.provider.label(),
                ctx.session_id,
                policy.timeout
            );
            fail_cancellation(ctx, message);
            None
        }
    }
}

pub(super) fn settle_cancelled_prompt(
    ctx: CancelCtx<'_>,
    response: acp::Result<acp::PromptResponse>,
) {
    match response {
        Ok(response) if response.stop_reason == acp::StopReason::Cancelled => {
            drop(ctx.permit);
            let _ = ctx.cancellation.response.send(Ok(()));
            dispatch_turn_terminal(ctx.events, ctx.session_id, "cancelled");
        }
        Ok(response) => {
            tracing::debug!(
                ?response.stop_reason,
                session_id = ctx.session_id,
                "ACP prompt completed while session cancellation was racing"
            );
            drop(ctx.permit);
            let _ = ctx.cancellation.response.send(Ok(()));
            dispatch_turn_terminal(ctx.events, ctx.session_id, "completed");
        }
        Err(error) => {
            log_prompt_settlement_error(ctx.provider, ctx.session_id, &error);
            drop(ctx.permit);
            let _ = ctx.cancellation.response.send(Ok(()));
            dispatch_turn_terminal(ctx.events, ctx.session_id, "cancelled");
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
pub(super) fn log_prompt_settlement_error(
    provider: AcpProvider,
    session_id: &str,
    error: &acp::Error,
) {
    tracing::debug!(
        provider = provider.label(),
        session_id,
        ?error,
        "ACP provider reported an error while settling an explicit cancellation"
    );
}

pub(super) fn fail_cancellation(ctx: CancelCtx<'_>, message: String) {
    ctx.invalidated_sessions
        .borrow_mut()
        .insert(ctx.session_id.to_owned());
    drop(ctx.permit);
    let _ = ctx
        .cancellation
        .response
        .send(Err(anyhow!(message.clone())));
    updates::dispatch_error(ctx.events, ctx.session_id, message);
}
