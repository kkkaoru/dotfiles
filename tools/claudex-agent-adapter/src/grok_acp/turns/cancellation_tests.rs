#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::grok_acp::client::AcpClient;
    use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

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
            (
                Err(acp::Error::internal_error()),
                "turn/completed",
                "cancelled",
                true,
            ),
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
        fail_cancellation(ctx, "cancel failed".to_owned());
        assert!(result.await.unwrap().is_err());
        assert!(invalidated.borrow().contains("session"));
        assert_eq!(receiver.recv().await.unwrap()["method"], "error");
    }

    #[tokio::test]
    async fn bounds_failed_and_stalled_cancel_requests() {
        for settlement in [
            Settlement::Settled(Err(acp::Error::internal_error())),
            Settlement::TimedOut,
        ] {
            let events = ThreadEventDispatcher::default();
            let receiver = events.subscribe("session");
            let invalidated = InvalidatedSessions::default();
            let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
            let (sender, result) = tokio::sync::oneshot::channel();
            let ctx = CancelCtx {
                provider: AcpProvider::Configured,
                session_id: "session",
                permit: permits.acquire_owned().await.unwrap(),
                cancellation: CancelRequest { response: sender },
                events: &events,
                invalidated_sessions: &invalidated,
            };
            assert!(
                continue_after_cancel_request(ctx, SettlementPolicy::default(), settlement)
                    .is_none()
            );
            assert!(result.await.unwrap().is_err());
            assert!(invalidated.borrow().contains("session"));
            assert_eq!(receiver.recv().await.unwrap()["method"], "error");
        }
    }

    #[tokio::test]
    async fn rejects_a_cancel_request_after_the_transport_closes() {
        let events = std::sync::Arc::new(ThreadEventDispatcher::default());
        let receiver = events.subscribe("session");
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (sender, result) = tokio::sync::oneshot::channel();
        let ctx = CancelCtx {
            provider: AcpProvider::Configured,
            session_id: "session",
            permit: permits.acquire_owned().await.unwrap(),
            cancellation: CancelRequest { response: sender },
            events: &events,
            invalidated_sessions: &invalidated,
        };
        let (outgoing, outgoing_peer) = tokio::io::duplex(64);
        let (incoming, incoming_peer) = tokio::io::duplex(64);
        drop(outgoing_peer);
        drop(incoming_peer);
        let connection = acp::ClientSideConnection::new(
            AcpClient::new(std::sync::Arc::clone(&events)),
            outgoing.compat_write(),
            incoming.compat(),
            drop,
        )
        .0;

        cancel_prompt(ctx, &connection, std::future::pending()).await;

        assert!(result.await.unwrap().is_err());
        assert!(invalidated.borrow().contains("session"));
        assert_eq!(receiver.recv().await.unwrap()["method"], "error");
    }
}
