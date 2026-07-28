use std::{
    future::Future,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

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

pub(super) fn invalidate(
    provider: AcpProvider,
    session_id: &str,
    permit: &mut Option<OwnedSemaphorePermit>,
    events: &ThreadEventDispatcher,
    active_turns: &ActiveTurns,
    invalidated_sessions: &InvalidatedSessions,
    alive: &AtomicBool,
    message: String,
) {
    debug_assert!(provider.is_session_scoped_configured());
    invalidated_sessions
        .borrow_mut()
        .insert(session_id.to_owned());
    drop(permit.take());
    active_turns.borrow_mut().remove(session_id);
    updates::dispatch_error(events, session_id, message);
    // RoutedBackend replaces dead providers before the next session/new. Dropping the retired
    // backend closes its command channel and terminates the old provider process group.
    alive.store(false, Ordering::Release);
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounds_only_session_scoped_configured_prompts() {
        assert!(matches!(
            wait(
                AcpProvider::Configured,
                Duration::from_millis(1),
                std::future::pending::<()>(),
            )
            .await,
            Wait::TimedOut
        ));
        assert!(matches!(
            wait(
                AcpProvider::Configured,
                Duration::from_secs(1),
                std::future::ready("completed"),
            )
            .await,
            Wait::Completed("completed")
        ));
        assert!(matches!(
            wait(
                AcpProvider::ConfiguredLaunchScoped,
                Duration::from_millis(1),
                std::future::ready("unchanged"),
            )
            .await,
            Wait::Completed("unchanged")
        ));
    }

    #[tokio::test]
    async fn invalidates_session_and_recycles_provider() {
        let events = ThreadEventDispatcher::default();
        let receiver = events.subscribe("session");
        let active = ActiveTurns::default();
        active.borrow_mut().insert("session".to_owned(), None);
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let alive = AtomicBool::new(true);

        invalidate(
            AcpProvider::Configured,
            "session",
            &mut permit,
            &events,
            &active,
            &invalidated,
            &alive,
            "configured prompt timed out".to_owned(),
        );

        assert!(!alive.load(Ordering::Acquire));
        assert!(invalidated.borrow().contains("session"));
        assert!(!active.borrow().contains_key("session"));
        assert!(permit.is_none());
        assert_eq!(receiver.recv().await.unwrap()["method"], "error");
    }

    #[tokio::test]
    async fn maps_prompt_completion_cancellation_and_failure() {
        for (response, expected_method, expected_status) in [
            (
                Ok(acp::PromptResponse::new(acp::StopReason::EndTurn)),
                "turn/completed",
                "completed",
            ),
            (
                Ok(acp::PromptResponse::new(acp::StopReason::Cancelled)),
                "turn/completed",
                "cancelled",
            ),
            (Err(acp::Error::internal_error()), "error", ""),
        ] {
            let events = ThreadEventDispatcher::default();
            let receiver = events.subscribe("session");
            finish(AcpProvider::Grok, "session", response, &events).await;
            let event = receiver.recv().await.unwrap();
            assert_eq!(event["method"], expected_method);
            assert_eq!(
                event
                    .pointer("/params/turn/status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(""),
                expected_status
            );
        }
    }
}
