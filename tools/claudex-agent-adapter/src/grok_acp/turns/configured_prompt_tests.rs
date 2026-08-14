use std::sync::{Arc, atomic::Ordering};

use super::*;
use crate::grok_acp::client::AcpClient;
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    task::LocalSet,
};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

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
        None,
    )
    .await;
    assert!(matches!(
        result,
        Wait::Quota(message) if message.contains("Weekly usage limit")
    ));
}

#[tokio::test]
async fn closed_quota_watch_falls_back_to_the_prompt_result() {
    let events = ThreadEventDispatcher::default();
    let activity = events.subscribe("session");
    let (tx, mut rx) = crate::grok_acp::stderr_quota::watch_channel();
    drop(tx);
    let result = wait_with_activity(
        AcpProvider::Configured,
        TIMEOUT,
        async {
            tokio::task::yield_now().await;
            "completed"
        },
        Some(activity),
        Some(&mut rx),
        None,
    )
    .await;
    assert!(matches!(result, Wait::Completed("completed")));
}

#[tokio::test]
async fn unbounded_wait_finishes_after_its_activity_stream_closes() {
    let activity = ThreadEvents::closed("session");
    let (release, continue_prompt) = tokio::sync::oneshot::channel();
    let waiter = tokio::spawn(wait_with_activity(
        AcpProvider::Grok,
        TIMEOUT,
        async {
            continue_prompt.await.expect("release prompt");
            "finished-after-close"
        },
        Some(activity),
        None,
        None,
    ));
    tokio::task::yield_now().await;
    release.send(()).expect("release waiter");
    let result = waiter.await.expect("waiter");
    assert!(matches!(result, Wait::Completed("finished-after-close")));
}

#[tokio::test]
async fn unbounded_wait_records_provider_activity_before_the_prompt_completes() {
    let events = Arc::new(ThreadEventDispatcher::default());
    let activity = events.subscribe("session");
    let saw_activity = Arc::new(AtomicBool::new(false));
    let (release, continue_prompt) = tokio::sync::oneshot::channel();
    let dispatch_events = Arc::clone(&events);
    let release_after_activity = tokio::spawn(async move {
        tokio::task::yield_now().await;
        dispatch_events.dispatch(serde_json::json!({
            "method":"item/providerTool/call",
            "params":{"threadId":"session","tool":"Read"}
        }));
        tokio::task::yield_now().await;
        release.send(()).expect("release prompt");
    });
    let result = wait_with_activity(
        AcpProvider::Grok,
        TIMEOUT,
        async {
            continue_prompt.await.expect("prompt release");
            "completed-after-activity"
        },
        Some(activity),
        None,
        Some(saw_activity.as_ref()),
    )
    .await;
    release_after_activity.await.expect("activity task");
    assert!(matches!(
        result,
        Wait::Completed("completed-after-activity")
    ));
    assert!(saw_activity.load(Ordering::Acquire));
}

#[tokio::test]
async fn configured_wait_times_out_immediately_for_a_closed_event_stream() {
    let result = wait_with_activity(
        AcpProvider::Configured,
        TIMEOUT,
        std::future::pending::<()>(),
        Some(ThreadEvents::closed("session")),
        None,
        None,
    )
    .await;
    assert!(matches!(result, Wait::TimedOut));
}

#[tokio::test]
async fn non_session_provider_skips_the_timeout_cancel_notification() {
    let events = Arc::new(ThreadEventDispatcher::default());
    let (outgoing, _outgoing_peer) = tokio::io::duplex(64);
    let (incoming, _incoming_peer) = tokio::io::duplex(64);
    let connection = acp::ClientSideConnection::new(
        AcpClient::new(events),
        outgoing.compat_write(),
        incoming.compat(),
        drop,
    )
    .0;
    cancel_timed_out_prompt(AcpProvider::Grok, &connection, "session").await;
}

#[tokio::test(flavor = "current_thread")]
async fn configured_timeout_cancel_tolerates_a_closed_transport() {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_test_writer()
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    LocalSet::new()
        .run_until(async {
            let events = Arc::new(ThreadEventDispatcher::default());
            let (outgoing, outgoing_peer) = tokio::io::duplex(64);
            let (incoming, incoming_peer) = tokio::io::duplex(64);
            drop(outgoing_peer);
            drop(incoming_peer);
            let (connection, io_task) = acp::ClientSideConnection::new(
                AcpClient::new(events),
                outgoing.compat_write(),
                incoming.compat(),
                |task| drop(tokio::task::spawn_local(task)),
            );
            spawn_connection_io(io_task);
            tokio::task::yield_now().await;
            cancel_timed_out_prompt(AcpProvider::Configured, &connection, "session").await;
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn configured_timeout_cancel_enqueues_an_open_transport_notification() {
    LocalSet::new()
        .run_until(async {
            let events = Arc::new(ThreadEventDispatcher::default());
            let (outgoing, outgoing_peer) = tokio::io::duplex(64);
            let (incoming, _incoming_peer) = tokio::io::duplex(64);
            let (connection, io_task) = acp::ClientSideConnection::new(
                AcpClient::new(events),
                outgoing.compat_write(),
                incoming.compat(),
                |task| drop(tokio::task::spawn_local(task)),
            );
            spawn_connection_io(io_task);
            let notification = spawn_cancel_notification_reader(outgoing_peer);

            cancel_timed_out_prompt(AcpProvider::Configured, &connection, "session").await;
            let notification = tokio::time::timeout(Duration::from_secs(1), notification)
                .await
                .expect("notification must be forwarded")
                .expect("notification sender");
            assert!(notification.contains("session/cancel"));
            assert!(notification.contains("session"));
        })
        .await;
}

fn spawn_connection_io(io_task: impl std::future::Future<Output = acp::Result<()>> + 'static) {
    drop(tokio::task::spawn_local(discard_connection_io(io_task)));
}

async fn discard_connection_io(io_task: impl std::future::Future<Output = acp::Result<()>>) {
    let _ = io_task.await;
}

fn spawn_cancel_notification_reader(
    outgoing_peer: tokio::io::DuplexStream,
) -> tokio::sync::oneshot::Receiver<String> {
    let (notification_sender, notification) = tokio::sync::oneshot::channel();
    drop(tokio::task::spawn_local(read_cancel_notification(
        outgoing_peer,
        notification_sender,
    )));
    notification
}

async fn read_cancel_notification(
    outgoing_peer: tokio::io::DuplexStream,
    notification_sender: tokio::sync::oneshot::Sender<String>,
) {
    let mut line = String::new();
    BufReader::new(outgoing_peer)
        .read_line(&mut line)
        .await
        .expect("cancel notification");
    notification_sender
        .send(line)
        .expect("notification receiver");
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
async fn non_cooldown_invalidation_keeps_the_provider_available() {
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
            trip_cooldown: false,
            message: "ordinary request failure".to_owned(),
        },
    );
    assert!(alive.load(Ordering::Acquire));
    assert!(!cooldown.load(Ordering::Acquire));
    assert!(invalidated.borrow().contains("session"));
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

#[test]
fn prompt_failure_uses_display_not_debug_dump() {
    let error = acp::Error::internal_error();
    let message = prompt_failure_message(AcpProvider::ConfiguredLaunchScoped, &error);
    assert!(message.contains("ConfiguredLaunch ACP prompt failed:"));
    assert!(!message.contains("Error {"));
    assert!(!message.contains("code:"));
}

#[test]
fn prompt_failure_labels_cline_credits_insufficient_balance() {
    let error = acp::Error::new(
        -32603,
        "Internal error: Insufficient balance. Add credits at https://app.cline.bot/credits",
    );
    let message = prompt_failure_message(AcpProvider::ConfiguredLaunchScoped, &error);
    assert!(message.contains("Cline ACP prompt failed"), "{message}");
    assert!(message.contains("Cline Credits"), "{message}");
    assert!(message.contains("Do not retry"), "{message}");
    assert!(!message.contains("ConfiguredLaunch"), "{message}");
    assert!(!message.contains("codex app-server"), "{message}");
    assert!(!message.contains("Error {"), "{message}");
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
