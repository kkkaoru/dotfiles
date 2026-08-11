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
        Invalidation {
            session_id: "session",
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
            alive: &alive,
            message: "configured prompt timed out".to_owned(),
        },
    );

    assert!(!alive.load(Ordering::Acquire));
    assert!(invalidated.borrow().contains("session"));
    assert!(!active.borrow().contains_key("session"));
    assert!(permit.is_none());
    assert_eq!(receiver.recv().await.unwrap()["method"], "error");
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
