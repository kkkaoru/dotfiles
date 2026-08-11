use std::{future::Future, sync::atomic::AtomicBool, time::Duration};

use agent_client_protocol as acp;
use tokio::sync::OwnedSemaphorePermit;

use super::{ActiveTurns, InvalidatedSessions, dispatch_turn_terminal};
use crate::{
    app_server::events::ThreadEventDispatcher,
    grok_acp::{connection::AcpProvider, updates},
};

pub(super) const TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub(super) enum Wait<T> {
    Completed(T),
    TimedOut,
}

pub(super) struct Invalidation<'a> {
    pub(super) session_id: &'a str,
    pub(super) permit: &'a mut Option<OwnedSemaphorePermit>,
    pub(super) events: &'a ThreadEventDispatcher,
    pub(super) active_turns: &'a ActiveTurns,
    pub(super) invalidated_sessions: &'a InvalidatedSessions,
    pub(super) alive: &'a AtomicBool,
    pub(super) message: String,
}

pub(super) async fn wait<T, F>(provider: AcpProvider, timeout: Duration, future: F) -> Wait<T>
where
    F: Future<Output = T>,
{
    if !provider.is_session_scoped_configured() {
        return Wait::Completed(future.await);
    }
    match tokio::time::timeout(timeout, future).await {
        Ok(output) => Wait::Completed(output),
        Err(_) => Wait::TimedOut,
    }
}

pub(super) fn invalidate(provider: AcpProvider, context: Invalidation<'_>) {
    let Invalidation {
        session_id,
        permit,
        events,
        active_turns,
        invalidated_sessions,
        alive,
        message,
    } = context;
    debug_assert!(provider.is_session_scoped_configured());
    invalidated_sessions
        .borrow_mut()
        .insert(session_id.to_owned());
    drop(permit.take());
    active_turns.borrow_mut().remove(session_id);
    updates::dispatch_error(events, session_id, message);
    // Session-scoped configured ACP: mark this session invalidated so later
    // prompts on the same id are rejected. Do NOT kill the shared stdio driver
    // (`alive=false`) — one timed-out SubAgent would respawn Cursor/OpenCode for
    // every other session on the route.
    let _ = alive;
}

pub(super) async fn finish(
    provider: AcpProvider,
    session_id: &str,
    response: acp::Result<acp::PromptResponse>,
    events: &ThreadEventDispatcher,
) {
    match response {
        Ok(response) => {
            // Notifications parsed before the response are local tasks; dispatch them first.
            tokio::task::yield_now().await;
            let status = if response.stop_reason == acp::StopReason::Cancelled {
                "cancelled"
            } else {
                "completed"
            };
            dispatch_turn_terminal(events, session_id, status);
        }
        Err(error) => updates::dispatch_error(
            events,
            session_id,
            format!("{} ACP prompt failed: {error:?}", provider.label()),
        ),
    }
}

#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "configured_prompt_tests.rs"]
mod tests;
