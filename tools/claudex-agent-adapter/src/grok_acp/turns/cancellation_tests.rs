#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn settles_cancelled_completed_and_failed_prompts() {
        for (response, expected_method, expected_status, succeeds) in [
            (
                Ok(acp::PromptResponse::new(acp::StopReason::Cancelled)),
                "turn/completed",
                "cancelled",
                true,
            ),
            (
                Ok(acp::PromptResponse::new(acp::StopReason::EndTurn)),
                "turn/completed",
                "completed",
                true,
            ),
            (Err(acp::Error::internal_error()), "error", "", false),
        ] {
            let events = ThreadEventDispatcher::default();
            let receiver = events.subscribe("session");
            let invalidated = InvalidatedSessions::default();
            let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
            let (sender, result) = tokio::sync::oneshot::channel();
            let ctx = CancelCtx {
                provider: AcpProvider::Grok,
                session_id: "session",
                permit: permits.acquire_owned().await.unwrap(),
                cancellation: CancelRequest { response: sender },
                events: &events,
                invalidated_sessions: &invalidated,
            };
            settle_cancelled_prompt(ctx, response);
            assert_eq!(result.await.unwrap().is_ok(), succeeds);
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

    #[tokio::test]
    async fn marks_direct_cancellation_failures_as_invalidated() {
        let events = ThreadEventDispatcher::default();
        let receiver = events.subscribe("session");
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (sender, result) = tokio::sync::oneshot::channel();
        let ctx = CancelCtx {
            provider: AcpProvider::Copilot,
            session_id: "session",
            permit: permits.acquire_owned().await.unwrap(),
            cancellation: CancelRequest { response: sender },
            events: &events,
            invalidated_sessions: &invalidated,
        };
        fail_cancellation(ctx, "cancel failed".to_owned(), true);
        assert!(result.await.unwrap().is_err());
        assert!(invalidated.borrow().contains("session"));
        assert_eq!(receiver.recv().await.unwrap()["method"], "error");
    }
}
