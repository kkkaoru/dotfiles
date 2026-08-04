#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::excessive_nesting)]
mod tests {
    use super::*;
    use crate::grok_acp::client::AcpClient;
    use serde_json::{Value, json};
    use tokio::{
        io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
        task::LocalSet,
    };
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    #[tokio::test]
    async fn finishes_pre_prompt_and_setup_cancellations() {
        assert_setup_cancellation(false).await;
        assert_setup_cancellation(true).await;
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
        ctl.finish_pre_prompt_cancel(CancelRequest { response });
        assert!(result.await.unwrap().is_ok());
        assert_eq!(
            receiver.recv().await.unwrap()["params"]["turn"]["status"],
            "cancelled"
        );
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
            TurnExecution {
                provider: AcpProvider::Grok,
                connection: std::rc::Rc::new(disconnected_connection(std::sync::Arc::clone(
                    &events,
                ))),
                model: "model",
                events: &events,
                active_turns: &active,
                invalidated_sessions: &invalidated,
                alive: &AtomicBool::new(true),
            },
            turn,
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
    async fn finishes_successful_effort_setup() {
        let events = ThreadEventDispatcher::default();
        let active = ActiveTurns::default();
        active.borrow_mut().insert("session".to_owned(), None);
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (_cancel_sender, mut cancellation) = oneshot::channel();
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
        assert!(finish_effort_setup(&mut ctl, Ok(())));
        assert!(permit.is_some());
        assert!(active.borrow().contains_key("session"));
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
            Err(EffortSetupError::Failed(acp::Error::internal_error()))
        ));
        assert!(result.await.unwrap().is_err());
        assert_eq!(receiver.recv().await.unwrap()["method"], "error");
    }

    #[tokio::test]
    async fn launch_scoped_configured_effort_failures_continue_the_turn() {
        let events = ThreadEventDispatcher::default();
        let active = ActiveTurns::default();
        active.borrow_mut().insert("session".to_owned(), None);
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (_cancel_sender, mut cancellation) = oneshot::channel();
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let mut ctl = TurnCtl {
            provider: AcpProvider::ConfiguredLaunchScoped,
            session_id: "session",
            cancellation: &mut cancellation,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };
        assert!(finish_effort_setup(
            &mut ctl,
            Err(EffortSetupError::Failed(acp::Error::internal_error()))
        ));
        assert!(finish_effort_setup(
            &mut ctl,
            Err(EffortSetupError::TimedOut)
        ));
        assert!(permit.is_some());
        assert!(active.borrow().contains_key("session"));
    }

    #[tokio::test]
    async fn effort_timeout_continues_native_providers() {
        let events = ThreadEventDispatcher::default();
        let active = ActiveTurns::default();
        active.borrow_mut().insert("session".to_owned(), None);
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (_cancel_sender, mut cancellation) = oneshot::channel();
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
        assert!(finish_effort_setup(
            &mut ctl,
            Err(EffortSetupError::TimedOut)
        ));
        assert!(permit.is_some());
        assert!(active.borrow().contains_key("session"));
    }

    #[tokio::test]
    async fn configured_effort_timeouts_fail_and_recycle_the_session() {
        let events = ThreadEventDispatcher::default();
        let receiver = events.subscribe("session");
        let active = ActiveTurns::default();
        active.borrow_mut().insert("session".to_owned(), None);
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (_cancel_sender, mut cancellation) = oneshot::channel();
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let mut ctl = TurnCtl {
            provider: AcpProvider::Configured,
            session_id: "session",
            cancellation: &mut cancellation,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };

        assert!(!finish_effort_setup(
            &mut ctl,
            Err(EffortSetupError::TimedOut)
        ));
        assert!(permit.is_none());
        assert!(!active.borrow().contains_key("session"));
        assert_eq!(receiver.recv().await.unwrap()["method"], "error");
    }

    #[tokio::test]
    async fn effort_recovery_honors_a_cancellation_that_arrives_after_failure() {
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

        assert!(!finish_effort_setup(
            &mut ctl,
            Err(EffortSetupError::TimedOut)
        ));
        assert!(result.await.unwrap().is_ok());
        assert!(permit.is_none());
        assert_eq!(
            receiver.recv().await.unwrap()["params"]["turn"]["status"],
            "cancelled"
        );
    }

    #[tokio::test]
    async fn skips_model_setup_when_the_provider_does_not_need_it() {
        assert_model_setup_skipped(AcpProvider::Grok).await;
        assert_model_setup_skipped(AcpProvider::ConfiguredLaunchScoped).await;
    }

    #[tokio::test]
    async fn configured_prompt_failures_invalidate_the_provider() {
        let events = std::sync::Arc::new(ThreadEventDispatcher::default());
        let receiver = events.subscribe("session");
        let active = ActiveTurns::default();
        active.borrow_mut().insert("session".to_owned(), None);
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (_cancel_sender, mut cancellation) = oneshot::channel();
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let ctl = TurnCtl {
            provider: AcpProvider::Configured,
            session_id: "session",
            cancellation: &mut cancellation,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };
        let alive = AtomicBool::new(true);

        run_prompt(
            ctl,
            std::rc::Rc::new(disconnected_connection(std::sync::Arc::clone(&events))),
            acp::SessionId::new("session".to_owned()),
            "prompt".to_owned(),
            configured_prompt::TIMEOUT,
            &alive,
        )
        .await;

        assert!(!alive.load(std::sync::atomic::Ordering::Acquire));
        assert!(invalidated.borrow().contains("session"));
        assert!(!active.borrow().contains_key("session"));
        assert!(permit.is_none());
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
            configured_prompt::TIMEOUT,
            &AtomicBool::new(true),
        )
        .await;
        assert_eq!(receiver.recv().await.unwrap()["method"], "error");
        assert!(!active.borrow().contains_key("session"));
        assert!(permit.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sends_effort_metadata_after_the_cancellation_channel_closes() {
        LocalSet::new().run_until(check_effort_metadata()).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn configured_without_effort_still_sets_the_model() {
        LocalSet::new()
            .run_until(check_configured_model_without_effort())
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn configured_effort_falls_back_when_the_option_is_rejected() {
        LocalSet::new()
            .run_until(check_configured_effort_fallback())
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn launch_scoped_effort_skips_per_turn_model_reselect() {
        LocalSet::new()
            .run_until(check_launch_scoped_effort_skips_model_reselect())
            .await;
    }

    #[tokio::test]
    async fn configured_model_selection_failure_is_reported() {
        let events = std::sync::Arc::new(ThreadEventDispatcher::default());
        let receiver = events.subscribe("session");
        let active = ActiveTurns::default();
        active.borrow_mut().insert("session".to_owned(), None);
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (_sender, mut cancellation) = oneshot::channel();
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let mut ctl = TurnCtl {
            provider: AcpProvider::Configured,
            session_id: "session",
            cancellation: &mut cancellation,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };
        assert!(!apply_effort(
            &mut ctl,
            &std::rc::Rc::new(disconnected_connection(std::sync::Arc::clone(&events))),
            "model",
            None,
            &acp::SessionId::new("session".to_owned()),
        )
        .await);
        assert!(permit.is_none());
        assert_eq!(receiver.recv().await.unwrap()["method"], "error");
    }

    #[tokio::test]
    async fn cancellation_wins_before_configured_effort_setup_starts() {
        let events = std::sync::Arc::new(ThreadEventDispatcher::default());
        let active = ActiveTurns::default();
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (cancel_sender, mut cancellation) = oneshot::channel();
        let (response, result) = oneshot::channel();
        assert!(cancel_sender.send(CancelRequest { response }).is_ok());
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let mut ctl = TurnCtl {
            provider: AcpProvider::Configured,
            session_id: "session",
            cancellation: &mut cancellation,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };
        assert!(!apply_effort(
            &mut ctl,
            &std::rc::Rc::new(disconnected_connection(std::sync::Arc::clone(&events))),
            "model",
            Some("high"),
            &acp::SessionId::new("session".to_owned()),
        )
        .await);
        assert!(result.await.unwrap().is_ok());
        assert!(permit.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn configured_prompt_timeout_recycles_the_provider() {
        LocalSet::new().run_until(check_prompt_timeout()).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn completes_a_prompt_without_a_queued_cancellation() {
        LocalSet::new().run_until(check_prompt_completion()).await;
    }

    async fn assert_setup_cancellation(setup_started: bool) {
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

    async fn assert_model_setup_skipped(provider: AcpProvider) {
        let events = std::sync::Arc::new(ThreadEventDispatcher::default());
        let active = ActiveTurns::default();
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (_cancel_sender, mut cancellation) = oneshot::channel();
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let mut ctl = TurnCtl {
            provider,
            session_id: "session",
            cancellation: &mut cancellation,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };
        assert!(
            apply_effort(
                &mut ctl,
                &std::rc::Rc::new(disconnected_connection(std::sync::Arc::clone(&events))),
                "model",
                None,
                &acp::SessionId::new("session".to_owned()),
            )
            .await
        );
        assert!(permit.is_some());
    }

    async fn check_effort_metadata() {
        let events = std::sync::Arc::new(ThreadEventDispatcher::default());
        let active = ActiveTurns::default();
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (sender, mut cancellation) = oneshot::channel();
        drop(sender);
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
        let (connection, request) = responding_connection(std::sync::Arc::clone(&events));
        assert!(
            apply_effort(
                &mut ctl,
                &std::rc::Rc::new(connection),
                "model",
                Some("high"),
                &acp::SessionId::new("session".to_owned()),
            )
            .await
        );
        let request = request.await.unwrap();
        assert_eq!(request["method"], "session/set_model");
        assert_eq!(request["params"]["_meta"]["reasoningEffort"], "high");
    }

    async fn check_configured_model_without_effort() {
        let events = std::sync::Arc::new(ThreadEventDispatcher::default());
        let active = ActiveTurns::default();
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (_sender, mut cancellation) = oneshot::channel();
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let mut ctl = TurnCtl {
            provider: AcpProvider::Configured,
            session_id: "session",
            cancellation: &mut cancellation,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };
        let (connection, request) = responding_connection(std::sync::Arc::clone(&events));
        assert!(apply_effort(
            &mut ctl,
            &std::rc::Rc::new(connection),
            "model",
            None,
            &acp::SessionId::new("session".to_owned()),
        )
        .await);
        assert_eq!(request.await.unwrap()["method"], "session/set_model");
    }

    async fn check_configured_effort_fallback() {
        let events = std::sync::Arc::new(ThreadEventDispatcher::default());
        let active = ActiveTurns::default();
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (_sender, mut cancellation) = oneshot::channel();
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let mut ctl = TurnCtl {
            provider: AcpProvider::Configured,
            session_id: "session",
            cancellation: &mut cancellation,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };
        let (connection, requests) = rejecting_effort_connection(std::sync::Arc::clone(&events));
        assert!(apply_effort(
            &mut ctl,
            &std::rc::Rc::new(connection),
            "model",
            Some("high"),
            &acp::SessionId::new("session".to_owned()),
        )
        .await);
        let requests = requests.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0]["method"], "session/set_model");
        assert!(requests[0].pointer("/params/_meta/reasoningEffort").is_none());
        assert_eq!(requests[1]["method"], "session/set_config_option");
        assert_eq!(requests[2]["method"], "session/set_model");
        assert_eq!(
            requests[2].pointer("/params/_meta/reasoningEffort"),
            Some(&json!("high"))
        );
    }

    async fn check_launch_scoped_effort_skips_model_reselect() {
        let events = std::sync::Arc::new(ThreadEventDispatcher::default());
        let active = ActiveTurns::default();
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (_sender, mut cancellation) = oneshot::channel();
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let mut ctl = TurnCtl {
            provider: AcpProvider::ConfiguredLaunchScoped,
            session_id: "session",
            cancellation: &mut cancellation,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };
        let (connection, requests) =
            rejecting_launch_scoped_effort_connection(std::sync::Arc::clone(&events));
        assert!(apply_effort(
            &mut ctl,
            &std::rc::Rc::new(connection),
            "auto",
            Some("high"),
            &acp::SessionId::new("session".to_owned()),
        )
        .await);
        let requests = requests.await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["method"], "session/set_config_option");
        assert!(permit.is_some());
    }

    async fn check_prompt_timeout() {
        let events = std::sync::Arc::new(ThreadEventDispatcher::default());
        let receiver = events.subscribe("session");
        let active = ActiveTurns::default();
        active.borrow_mut().insert("session".to_owned(), None);
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (_sender, mut cancellation) = oneshot::channel();
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let (connection, _outgoing, _incoming) = stalled_connection(std::sync::Arc::clone(&events));
        let ctl = TurnCtl {
            provider: AcpProvider::Configured,
            session_id: "session",
            cancellation: &mut cancellation,
            permit: &mut permit,
            events: &events,
            active_turns: &active,
            invalidated_sessions: &invalidated,
        };
        let alive = AtomicBool::new(true);
        run_prompt(
            ctl,
            std::rc::Rc::new(connection),
            acp::SessionId::new("session".to_owned()),
            "prompt".to_owned(),
            Duration::from_millis(1),
            &alive,
        )
        .await;
        assert!(!alive.load(std::sync::atomic::Ordering::Acquire));
        assert!(invalidated.borrow().contains("session"));
        assert!(!active.borrow().contains_key("session"));
        assert!(permit.is_none());
        assert_eq!(receiver.recv().await.unwrap()["method"], "error");
    }

    async fn check_prompt_completion() {
        let events = std::sync::Arc::new(ThreadEventDispatcher::default());
        let receiver = events.subscribe("session");
        let active = ActiveTurns::default();
        active.borrow_mut().insert("session".to_owned(), None);
        let invalidated = InvalidatedSessions::default();
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let (_sender, mut cancellation) = oneshot::channel();
        let mut permit = Some(permits.acquire_owned().await.unwrap());
        let (connection, request) = responding_connection(std::sync::Arc::clone(&events));
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
            std::rc::Rc::new(connection),
            acp::SessionId::new("session".to_owned()),
            "prompt".to_owned(),
            configured_prompt::TIMEOUT,
            &AtomicBool::new(true),
        )
        .await;
        assert_eq!(request.await.unwrap()["method"], "session/prompt");
        assert_eq!(
            receiver.recv().await.unwrap()["params"]["turn"]["status"],
            "completed"
        );
        assert!(permit.is_none());
    }

    async fn settled_setup() {}

    async fn pending_prompt() -> acp::Result<acp::PromptResponse> {
        std::future::pending().await
    }

    fn responding_connection(
        events: std::sync::Arc<ThreadEventDispatcher>,
    ) -> (acp::ClientSideConnection, oneshot::Receiver<Value>) {
        let (outgoing, outgoing_peer) = tokio::io::duplex(1024);
        let (incoming, mut incoming_peer) = tokio::io::duplex(1024);
        let (connection, io_task) = acp::ClientSideConnection::new(
            AcpClient::new(events),
            outgoing.compat_write(),
            incoming.compat(),
            |task| {
                drop(tokio::task::spawn_local(task));
            },
        );
        drop(tokio::task::spawn_local(async move {
            let _ = io_task.await;
        }));
        let (request_sender, request) = oneshot::channel();
        drop(tokio::task::spawn_local(async move {
            let mut line = String::new();
            BufReader::new(outgoing_peer)
                .read_line(&mut line)
                .await
                .expect("ACP request");
            let request: Value = serde_json::from_str(&line).expect("valid ACP request");
            let id = request["id"].clone();
            request_sender
                .send(request.clone())
                .expect("request receiver");
            let result = match request["method"].as_str() {
                Some("session/set_model") => json!({}),
                Some("session/prompt") => json!({"stopReason":"end_turn"}),
                method => panic!("unexpected ACP method: {method:?}"),
            };
            let response = json!({"jsonrpc":"2.0", "id":id, "result":result}).to_string();
            incoming_peer
                .write_all(response.as_bytes())
                .await
                .expect("ACP response");
            incoming_peer
                .write_all(b"\n")
                .await
                .expect("response newline");
        }));
        (connection, request)
    }

    fn rejecting_effort_connection(
        events: std::sync::Arc<ThreadEventDispatcher>,
    ) -> (acp::ClientSideConnection, oneshot::Receiver<Vec<Value>>) {
        let (outgoing, outgoing_peer) = tokio::io::duplex(1024);
        let (incoming, mut incoming_peer) = tokio::io::duplex(1024);
        let (connection, io_task) = acp::ClientSideConnection::new(
            AcpClient::new(events),
            outgoing.compat_write(),
            incoming.compat(),
            |task| {
                drop(tokio::task::spawn_local(task));
            },
        );
        drop(tokio::task::spawn_local(async move {
            let _ = io_task.await;
        }));
        let (request_sender, requests) = oneshot::channel();
        drop(tokio::task::spawn_local(async move {
            let mut lines = BufReader::new(outgoing_peer);
            let mut captured = Vec::new();
            // model select → rejected effort option → model meta fallback
            for _ in 0..3 {
                let mut line = String::new();
                lines.read_line(&mut line).await.expect("ACP request");
                let request: Value = serde_json::from_str(&line).expect("valid ACP request");
                let id = request["id"].clone();
                let response = match request["method"].as_str() {
                    Some("session/set_config_option") => {
                        json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32602,"message":"invalid params"}})
                    }
                    Some("session/set_model") => {
                        json!({"jsonrpc":"2.0", "id":id, "result":{}})
                    }
                    method => panic!("unexpected ACP method: {method:?}"),
                };
                captured.push(request);
                incoming_peer
                    .write_all(response.to_string().as_bytes())
                    .await
                    .expect("ACP response");
                incoming_peer
                    .write_all(b"\n")
                    .await
                    .expect("response newline");
            }
            request_sender.send(captured).expect("request receiver");
        }));
        (connection, requests)
    }

    fn rejecting_launch_scoped_effort_connection(
        events: std::sync::Arc<ThreadEventDispatcher>,
    ) -> (acp::ClientSideConnection, oneshot::Receiver<Vec<Value>>) {
        let (outgoing, outgoing_peer) = tokio::io::duplex(1024);
        let (incoming, mut incoming_peer) = tokio::io::duplex(1024);
        let (connection, io_task) = acp::ClientSideConnection::new(
            AcpClient::new(events),
            outgoing.compat_write(),
            incoming.compat(),
            |task| {
                drop(tokio::task::spawn_local(task));
            },
        );
        drop(tokio::task::spawn_local(async move {
            let _ = io_task.await;
        }));
        let (request_sender, requests) = oneshot::channel();
        drop(tokio::task::spawn_local(async move {
            let mut lines = BufReader::new(outgoing_peer);
            let mut line = String::new();
            lines.read_line(&mut line).await.expect("ACP request");
            let request: Value = serde_json::from_str(&line).expect("valid ACP request");
            assert_eq!(request["method"], "session/set_config_option");
            let id = request["id"].clone();
            let response = json!({
                "jsonrpc":"2.0",
                "id":id,
                "error":{"code":-32602,"message":"invalid params"}
            });
            incoming_peer
                .write_all(response.to_string().as_bytes())
                .await
                .expect("ACP response");
            incoming_peer
                .write_all(b"\n")
                .await
                .expect("response newline");
            request_sender
                .send(vec![request])
                .expect("request receiver");
        }));
        (connection, requests)
    }

    fn stalled_connection(
        events: std::sync::Arc<ThreadEventDispatcher>,
    ) -> (
        acp::ClientSideConnection,
        tokio::io::DuplexStream,
        tokio::io::DuplexStream,
    ) {
        let (outgoing, outgoing_peer) = tokio::io::duplex(1024);
        let (incoming, incoming_peer) = tokio::io::duplex(1024);
        let (connection, io_task) = acp::ClientSideConnection::new(
            AcpClient::new(events),
            outgoing.compat_write(),
            incoming.compat(),
            |task| {
                drop(tokio::task::spawn_local(task));
            },
        );
        drop(tokio::task::spawn_local(async move {
            let _ = io_task.await;
        }));
        (connection, outgoing_peer, incoming_peer)
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
