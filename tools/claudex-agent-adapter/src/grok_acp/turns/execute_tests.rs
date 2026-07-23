#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::grok_acp::client::AcpClient;
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    #[tokio::test]
    async fn finishes_pre_prompt_and_setup_cancellations() {
        for setup_started in [false, true] {
            let events = ThreadEventDispatcher::default();
            let receiver = events.subscribe("session");
            let active = ActiveTurns::default();
            active.borrow_mut().insert("session".to_owned(), None);
            let invalidated = InvalidatedSessions::default();
            let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
            let (_cancel_sender, mut cancel_receiver) = oneshot::channel();
            let mut permit = Some(permits.acquire_owned().await.unwrap());
            let (response, result) = oneshot::channel();
            let mut ctl = TurnCtl {
                provider: AcpProvider::Grok,
                session_id: "session",
                cancellation: &mut cancel_receiver,
                permit: &mut permit,
                events: &events,
                active_turns: &active,
                invalidated_sessions: &invalidated,
            };
            handle_setup_cancellation(
                &mut ctl,
                setup_started,
                settled_setup(),
                CancelRequest { response },
            )
            .await;
            assert!(result.await.unwrap().is_ok());
            assert_eq!(
                receiver.recv().await.unwrap()["params"]["turn"]["status"],
                "cancelled"
            );
            assert!(permit.is_none());
        }
    }

    #[tokio::test]
    async fn direct_pre_prompt_cancel_uses_the_same_terminal_path() {
        let events = ThreadEventDispatcher::default();
        let receiver = events.subscribe("session");
        let active = ActiveTurns::default();
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (_cancel_sender, mut cancel_receiver) = oneshot::channel();
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let (response, result) = oneshot::channel();
        let mut ctl = TurnCtl {
            provider: AcpProvider::Copilot,
            session_id: "session",
            cancellation: &mut cancel_receiver,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };
        finish_unstarted_prompt(&mut ctl, CancelRequest { response });
        assert!(result.await.unwrap().is_ok());
        assert_eq!(
            receiver.recv().await.unwrap()["params"]["turn"]["status"],
            "cancelled"
        );
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
            finish_prompt(AcpProvider::Grok, "session", response, &events).await;
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
    async fn handles_cancellation_before_a_prompt_future_starts() {
        let events = std::sync::Arc::new(ThreadEventDispatcher::default());
        let receiver = events.subscribe("session");
        let active = ActiveTurns::default();
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (_cancel_sender, mut cancel_receiver) = oneshot::channel();
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let (response, result) = oneshot::channel();
        let mut ctl = TurnCtl {
            provider: AcpProvider::Grok,
            session_id: "session",
            cancellation: &mut cancel_receiver,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };
        let connection = disconnected_connection(std::sync::Arc::clone(&events));
        handle_prompt_cancellation(
            &mut ctl,
            &connection,
            false,
            pending_prompt(),
            CancelRequest { response },
        )
        .await;
        assert!(result.await.unwrap().is_ok());
        assert_eq!(
            receiver.recv().await.unwrap()["params"]["turn"]["status"],
            "cancelled"
        );
    }

    #[tokio::test]
    async fn executes_a_cancellation_that_was_already_queued() {
        let events = std::sync::Arc::new(ThreadEventDispatcher::default());
        let receiver = events.subscribe("session");
        let active = ActiveTurns::default();
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (cancel_sender, cancel_receiver) = oneshot::channel();
        let (response, result) = oneshot::channel();
        assert!(cancel_sender.send(CancelRequest { response }).is_ok());
        let turn = PreparedTurn {
            session_id: "session".to_owned(),
            prompt: "unused".to_owned(),
            effort: None,
            cancellation: cancel_receiver,
            _permit: permits.acquire_owned().await.unwrap(),
        };
        execute_turn(
            AcpProvider::Grok,
            std::rc::Rc::new(disconnected_connection(std::sync::Arc::clone(&events))),
            "model",
            turn,
            &events,
            &active,
            &invalidated,
        )
        .await;
        assert!(result.await.unwrap().is_ok());
        assert_eq!(
            receiver.recv().await.unwrap()["params"]["turn"]["status"],
            "cancelled"
        );
    }

    #[tokio::test]
    async fn finishes_effort_setup_with_a_queued_cancellation() {
        let events = ThreadEventDispatcher::default();
        let receiver = events.subscribe("session");
        let active = ActiveTurns::default();
        active.borrow_mut().insert("session".to_owned(), None);
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (cancel_sender, mut cancellation) = oneshot::channel();
        let (response, result) = oneshot::channel();
        assert!(cancel_sender.send(CancelRequest { response }).is_ok());
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let mut ctl = TurnCtl {
            provider: AcpProvider::Grok,
            session_id: "session",
            cancellation: &mut cancellation,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };
        assert!(!finish_effort_setup(&mut ctl, Ok(())));
        assert!(result.await.unwrap().is_ok());
        assert_eq!(
            receiver.recv().await.unwrap()["params"]["turn"]["status"],
            "cancelled"
        );
    }

    #[tokio::test]
    async fn reports_effort_failure_to_a_queued_cancellation() {
        let events = ThreadEventDispatcher::default();
        let receiver = events.subscribe("session");
        let active = ActiveTurns::default();
        active.borrow_mut().insert("session".to_owned(), None);
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (cancel_sender, mut cancellation) = oneshot::channel();
        let (response, result) = oneshot::channel();
        assert!(cancel_sender.send(CancelRequest { response }).is_ok());
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let mut ctl = TurnCtl {
            provider: AcpProvider::Copilot,
            session_id: "session",
            cancellation: &mut cancellation,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };
        assert!(!finish_effort_setup(
            &mut ctl,
            Err(acp::Error::internal_error())
        ));
        assert!(result.await.unwrap().is_err());
        assert_eq!(receiver.recv().await.unwrap()["method"], "error");
    }

    #[tokio::test]
    async fn awaits_the_prompt_when_the_cancellation_channel_closes() {
        let events = std::sync::Arc::new(ThreadEventDispatcher::default());
        let receiver = events.subscribe("session");
        let active = ActiveTurns::default();
        active.borrow_mut().insert("session".to_owned(), None);
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (cancel_sender, mut cancellation) = oneshot::channel();
        drop(cancel_sender);
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let ctl = TurnCtl {
            provider: AcpProvider::Grok,
            session_id: "session",
            cancellation: &mut cancellation,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };
        run_prompt(
            ctl,
            std::rc::Rc::new(disconnected_connection(std::sync::Arc::clone(&events))),
            acp::SessionId::new("session".to_owned()),
            "prompt".to_owned(),
        )
        .await;
        assert_eq!(receiver.recv().await.unwrap()["method"], "error");
        assert!(!active.borrow().contains_key("session"));
        assert!(permit.is_none());
    }

    async fn settled_setup() {}

    async fn pending_prompt() -> acp::Result<acp::PromptResponse> {
        std::future::pending().await
    }

    fn disconnected_connection(
        events: std::sync::Arc<ThreadEventDispatcher>,
    ) -> acp::ClientSideConnection {
        let (outgoing, _outgoing_peer) = tokio::io::duplex(64);
        let (incoming, _incoming_peer) = tokio::io::duplex(64);
        acp::ClientSideConnection::new(
            AcpClient::new(events),
            outgoing.compat_write(),
            incoming.compat(),
            drop,
        )
        .0
    }
}
