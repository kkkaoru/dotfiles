use std::{error::Error, fmt, future::Future, time::Duration};

use agent_client_protocol::{self as acp, Agent as _};
use anyhow::anyhow;

use super::{ActiveTurns, CancelRequest, InvalidatedSessions, dispatch_turn_terminal};
use crate::{
    app_server::events::ThreadEventDispatcher,
    grok_acp::{connection::AcpProvider, updates},
};

const CANCELLATION_SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) struct CancelCtx<'a> {
    pub(super) provider: AcpProvider,
    pub(super) session_id: &'a str,
    pub(super) permit: tokio::sync::OwnedSemaphorePermit,
    pub(super) cancellation: CancelRequest,
    pub(super) events: &'a ThreadEventDispatcher,
    pub(super) invalidated_sessions: &'a InvalidatedSessions,
}

#[derive(Clone, Copy)]
struct SettlementPolicy {
    timeout: Duration,
}

impl Default for SettlementPolicy {
    fn default() -> Self {
        Self {
            timeout: CANCELLATION_SETTLEMENT_TIMEOUT,
        }
    }
}

enum Settlement<T> {
    Settled(T),
    TimedOut,
}

impl SettlementPolicy {
    async fn settle<F, T>(self, future: F) -> Settlement<T>
    where
        F: Future<Output = T>,
    {
        match tokio::time::timeout(self.timeout, future).await {
            Ok(value) => Settlement::Settled(value),
            Err(_) => Settlement::TimedOut,
        }
    }
}

#[derive(Debug)]
struct CancellationSettlementTimeout {
    provider: AcpProvider,
    session_id: String,
    timeout: Duration,
}

impl fmt::Display for CancellationSettlementTimeout {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ACP session `{}` cancellation did not settle within {:?}",
            self.provider.label(),
            self.session_id,
            self.timeout
        )
    }
}

impl Error for CancellationSettlementTimeout {}

#[derive(Debug)]
struct SetupCancellationSettlementTimeout {
    provider: AcpProvider,
    session_id: String,
    timeout: Duration,
}

impl fmt::Display for SetupCancellationSettlementTimeout {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ACP session `{}` setup cancellation did not settle within {:?}",
            self.provider.label(),
            self.session_id,
            self.timeout
        )
    }
}

impl Error for SetupCancellationSettlementTimeout {}

pub(super) fn finish_setup_cancellation(
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

pub(super) async fn cancel_setup<F, T>(ctx: CancelCtx<'_>, active_turns: &ActiveTurns, setup: F)
where
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

pub(super) async fn cancel_prompt<F>(
    ctx: CancelCtx<'_>,
    connection: &acp::ClientSideConnection,
    prompt: F,
) where
    F: Future<Output = acp::Result<acp::PromptResponse>>,
{
    if let Err(error) = connection
        .cancel(acp::CancelNotification::new(ctx.session_id.to_owned()))
        .await
    {
        let message = format!(
            "{} ACP session/cancel failed: {error:?}",
            ctx.provider.label()
        );
        fail_cancellation(ctx, message, true);
        return;
    }
    let policy = SettlementPolicy::default();
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

fn settle_cancelled_prompt(ctx: CancelCtx<'_>, response: acp::Result<acp::PromptResponse>) {
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
            let message = format!(
                "{} ACP cancelled prompt failed to settle: {error:?}",
                ctx.provider.label()
            );
            fail_cancellation(ctx, message, false);
        }
    }
}

fn fail_cancellation(ctx: CancelCtx<'_>, message: String, invalidate: bool) {
    if invalidate {
        ctx.invalidated_sessions
            .borrow_mut()
            .insert(ctx.session_id.to_owned());
    }
    drop(ctx.permit);
    let _ = ctx
        .cancellation
        .response
        .send(Err(anyhow!(message.clone())));
    updates::dispatch_error(ctx.events, ctx.session_id, message);
}

#[cfg(test)]
include!("cancellation_tests.rs");
