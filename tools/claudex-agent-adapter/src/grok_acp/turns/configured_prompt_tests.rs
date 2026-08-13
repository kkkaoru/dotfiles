use std::sync::atomic::Ordering;

use super::*;

#[test]
fn keeps_the_production_no_event_budget_at_sixty_seconds() {
    assert_eq!(TIMEOUT, Duration::from_secs(60));
    assert_eq!(timeout(), TIMEOUT);
}

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

#[tokio::test(start_paused = true)]
async fn true_no_response_expires_after_the_logical_budget() {
    let events = ThreadEventDispatcher::default();
    let activity = events.subscribe("session");
    let task = tokio::spawn(wait_with_activity(
        AcpProvider::Configured,
        TIMEOUT,
        std::future::pending::<()>(),
        Some(activity),
        None,
    ));
    tokio::task::yield_now().await;
    tokio::time::advance(TIMEOUT - Duration::from_secs(1)).await;
    assert!(!task.is_finished(), "no-response must not expire early");
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(matches!(task.await.unwrap(), Wait::TimedOut));
}

#[tokio::test(start_paused = true)]
async fn provider_activity_resets_the_no_event_budget() {
    let events = ThreadEventDispatcher::default();
    let activity = events.subscribe("session");
    let task = tokio::spawn(wait_with_activity(
        AcpProvider::Configured,
        TIMEOUT,
        std::future::pending::<()>(),
        Some(activity),
        None,
    ));
    tokio::task::yield_now().await;
    tokio::time::advance(TIMEOUT - Duration::from_secs(1)).await;
    events.dispatch(serde_json::json!({
        "method":"item/providerTool/call",
        "params":{"threadId":"session","tool":"Read"}
    }));
    tokio::time::advance(TIMEOUT - Duration::from_secs(1)).await;
    assert!(
        !task.is_finished(),
        "provider activity should reset the timer"
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(matches!(task.await.unwrap(), Wait::TimedOut));
}

#[tokio::test]
async fn quota_watch_completes_immediately_without_the_no_event_budget() {
    let events = ThreadEventDispatcher::default();
    let activity = events.subscribe("session");
    let (tx, mut rx) = crate::grok_acp::stderr_quota::watch_channel();
    tx.send(Some("Weekly usage limit reached".to_owned()))
        .expect("quota");
    let result = wait_with_activity(
        AcpProvider::Configured,
        TIMEOUT,
        std::future::pending::<()>(),
        Some(activity),
        Some(&mut rx),
    )
    .await;
    assert!(matches!(
        result,
        Wait::Quota(message) if message.contains("Weekly usage limit")
    ));
}

#[tokio::test]
async fn invalidates_session_without_killing_shared_provider() {
    let events = ThreadEventDispatcher::default();
    let receiver = events.subscribe("session");
    let active = ActiveTurns::default();
    active.borrow_mut().insert("session".to_owned(), None);
    let invalidated = InvalidatedSessions::default();
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let mut permit = Some(permits.acquire_owned().await.unwrap());
    let alive = AtomicBool::new(true);
    let cooldown = AtomicBool::new(false);

    invalidate(
        AcpProvider::Configured,
        Invalidation {
            session_id: "session",
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
            alive: &alive,
            cooldown: &cooldown,
            trip_cooldown: true,
            message: "configured prompt timed out".to_owned(),
        },
    );

    assert!(alive.load(Ordering::Acquire), "driver must stay alive");
    assert!(invalidated.borrow().contains("session"));
    assert!(!active.borrow().contains_key("session"));
    assert!(permit.is_none());
    assert!(cooldown.load(Ordering::Acquire));
    assert_eq!(receiver.recv().await.unwrap()["method"], "error");
}

#[tokio::test]
async fn timeout_cooldown_is_provider_scoped_and_does_not_close_the_driver() {
    let events = ThreadEventDispatcher::default();
    let active = ActiveTurns::default();
    active.borrow_mut().insert("session".to_owned(), None);
    let invalidated = InvalidatedSessions::default();
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let mut permit = Some(permits.acquire_owned().await.unwrap());
    let alive = AtomicBool::new(true);
    let cooldown = AtomicBool::new(false);
    invalidate(
        AcpProvider::Configured,
        Invalidation {
            session_id: "session",
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
            alive: &alive,
            cooldown: &cooldown,
            trip_cooldown: true,
            message: "no event".to_owned(),
        },
    );
    assert!(alive.load(Ordering::Acquire));
    assert!(cooldown.load(Ordering::Acquire));
    assert!(!invalidated.borrow().is_empty());
}

#[tokio::test]
async fn maps_prompt_completion_cancellation_and_failure() {
    assert_prompt_finish(
        Ok(acp::PromptResponse::new(acp::StopReason::EndTurn)),
        "turn/completed",
        "completed",
    )
    .await;
    assert_prompt_finish(
        Ok(acp::PromptResponse::new(acp::StopReason::Cancelled)),
        "turn/completed",
        "cancelled",
    )
    .await;
    assert_prompt_finish(Err(acp::Error::internal_error()), "error", "").await;
}

async fn assert_prompt_finish(
    response: acp::Result<acp::PromptResponse>,
    expected_method: &str,
    expected_status: &str,
) {
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
