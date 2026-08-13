use std::{
    future::Future,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use agent_client_protocol::{self as acp, Agent as _};
use tokio::sync::OwnedSemaphorePermit;

use super::{ActiveTurns, InvalidatedSessions, dispatch_turn_terminal};
use crate::{
    app_server::{ThreadEvents, events::ThreadEventDispatcher},
    grok_acp::{connection::AcpProvider, updates},
};

/// Bound only the period with no ACP activity so productive long-running
/// configured turns retain their existing behavior.
pub(super) const TIMEOUT: Duration = Duration::from_secs(60);
const TIMEOUT_ENV: &str = "CLAUDEX_TEST_CONFIGURED_ACP_NO_EVENT_TIMEOUT_SECONDS";

pub(super) fn timeout() -> Duration {
    std::env::var(TIMEOUT_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map_or(TIMEOUT, Duration::from_secs)
}

pub(super) enum Wait<T> {
    Completed(T),
    TimedOut,
    Quota(String),
}

pub(super) struct Invalidation<'a> {
    pub(super) session_id: &'a str,
    pub(super) permit: &'a mut Option<OwnedSemaphorePermit>,
    pub(super) events: &'a ThreadEventDispatcher,
    pub(super) active_turns: &'a ActiveTurns,
    pub(super) invalidated_sessions: &'a InvalidatedSessions,
    pub(super) alive: &'a AtomicBool,
    pub(super) cooldown: &'a AtomicBool,
    pub(super) trip_cooldown: bool,
    pub(super) message: String,
}

#[cfg(test)]
pub(super) async fn wait<T, F>(provider: AcpProvider, timeout: Duration, future: F) -> Wait<T>
where
    F: Future<Output = T>,
{
    wait_with_activity(provider, timeout, future, None, None).await
}

pub(super) async fn wait_with_activity<T, F>(
    provider: AcpProvider,
    timeout: Duration,
    future: F,
    activity: Option<ThreadEvents>,
    mut quota: Option<&mut tokio::sync::watch::Receiver<Option<String>>>,
) -> Wait<T>
where
    F: Future<Output = T>,
{
    if !provider.is_session_scoped_configured() {
        return Wait::Completed(future.await);
    }

    let Some(activity) = activity else {
        return match tokio::time::timeout(timeout, future).await {
            Ok(output) => Wait::Completed(output),
            Err(_) => Wait::TimedOut,
        };
    };
    tokio::pin!(future);
    let timer = tokio::time::sleep(timeout);
    tokio::pin!(timer);
    loop {
        tokio::select! {
            biased;
            output = &mut future => return Wait::Completed(output),
            event = activity.recv() => {
                if event.is_none() {
                    return Wait::TimedOut;
                }
                timer.as_mut().reset(tokio::time::Instant::now() + timeout);
            }
            message = crate::grok_acp::stderr_quota::wait_quota_message(quota.as_deref_mut()) => {
                match message {
                    Some(message) => return Wait::Quota(message),
                    None => quota = None,
                }
            }
            () = &mut timer => return Wait::TimedOut,
        }
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
        cooldown,
        trip_cooldown,
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
    // prompts on the same id are rejected. Trip only this model/provider's
    // circuit; do NOT kill the shared stdio driver (`alive=false`) — one
    // timed-out SubAgent must not terminate already-running sibling turns.
    if trip_cooldown {
        cooldown.store(true, Ordering::Release);
    }
    let _ = alive;
}

pub(super) async fn cancel_timed_out_prompt(
    provider: AcpProvider,
    connection: &acp::ClientSideConnection,
    session_id: &str,
) {
    if !provider.is_session_scoped_configured() {
        return;
    }
    // The prompt future is dropped by the timeout, but ACP still owns the
    // server-side turn. Send one exact-session cancel and bound its response;
    // the timeout path itself remains the sole terminal error emitter.
    let cancel = connection.cancel(acp::CancelNotification::new(session_id.to_owned()));
    match tokio::time::timeout(Duration::from_secs(2), cancel).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(
            provider = provider.label(),
            session_id,
            ?error,
            "configured ACP timeout cancellation failed"
        ),
        Err(_) => tracing::warn!(
            provider = provider.label(),
            session_id,
            "configured ACP timeout cancellation did not settle"
        ),
    }
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
        Err(error) => {
            updates::dispatch_error(events, session_id, prompt_failure_message(provider, &error))
        }
    }
}

pub(super) fn prompt_failure_message(provider: AcpProvider, error: &acp::Error) -> String {
    let detail = error.to_string();
    if crate::anthropic::contains_cline_credits_balance_marker(&detail) {
        return crate::anthropic::cline_credits_failure_message(&detail);
    }
    format!("{} ACP prompt failed: {detail}", provider.label())
}

#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "configured_prompt_tests.rs"]
mod tests;
