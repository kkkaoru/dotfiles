use super::*;
use crate::grok_acp::client::AcpClient;
use anyhow::anyhow;
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

#[test]
fn replace_settle_timeout_stays_tight_for_mid_turn_steering() {
    assert_eq!(REPLACE_SETTLE_TIMEOUT, Duration::from_millis(200));
    assert!(
        REPLACE_SETTLE_TIMEOUT <= Duration::from_millis(250),
        "same-session replace must not stall Claude Code mid-turn steering"
    );
}

#[tokio::test(start_paused = true)]
async fn replace_shares_one_settle_budget_when_worker_never_exits() {
    let active_turns = ActiveTurns::default();
    let (cancel, cancel_receiver) = oneshot::channel();
    active_turns
        .borrow_mut()
        .insert("session".to_owned(), Some(cancel));
    // Accept the cancel request but never ack or clear active_turns so both
    // phases compete for the same REPLACE_SETTLE_TIMEOUT budget.
    let hold = tokio::spawn(async move {
        let _ = cancel_receiver.await;
        std::future::pending::<()>().await;
    });

    let started = tokio::time::Instant::now();
    let replace = replace_active_turn(AcpProvider::Grok, &active_turns, "session");
    let advance = async {
        tokio::time::sleep(REPLACE_SETTLE_TIMEOUT + Duration::from_millis(25)).await;
    };
    let (result, ()) = tokio::join!(replace, advance);
    let elapsed = started.elapsed();
    hold.abort();

    assert!(
        result.is_err(),
        "stuck replace must fail closed: {result:?}"
    );
    assert!(
        elapsed < REPLACE_SETTLE_TIMEOUT.saturating_mul(2),
        "stacked cancel+clear budgets would approach 2x; elapsed={elapsed:?}"
    );
    assert!(
        elapsed <= REPLACE_SETTLE_TIMEOUT + Duration::from_millis(50),
        "replace must release within one shared settle budget; elapsed={elapsed:?}"
    );
}

#[test]
fn prepares_prefixed_prompts_and_provider_specific_effort() {
    let instructions = Rc::new(RefCell::new(HashMap::from([(
        "session".to_owned(),
        "prefix".to_owned(),
    )])));
    let permits = Arc::new(tokio::sync::Semaphore::new(3));
    let turn = prepare_turn(
        AcpProvider::Grok,
        json!({"threadId":"session", "model":"model-a", "input":"prompt", "effort":"mid"}),
        Arc::clone(&permits).try_acquire_owned().unwrap(),
        oneshot::channel().1,
        &instructions,
    )
    .unwrap();
    assert_eq!(turn.model, "model-a");
    assert_eq!(turn.prompt, "prefix\n\nprompt");
    assert_eq!(turn.effort, None);
    assert!(instructions.borrow().is_empty());

    let copilot = prepare_turn(
        AcpProvider::Copilot,
        json!({"threadId":"copilot", "effort":"xhigh"}),
        Arc::clone(&permits).try_acquire_owned().unwrap(),
        oneshot::channel().1,
        &instructions,
    )
    .unwrap();
    assert_eq!(copilot.prompt, "");
    assert_eq!(copilot.effort.as_deref(), Some("xhigh"));

    let missing_thread = prepare_turn(
        AcpProvider::Configured,
        json!({"input":"prompt"}),
        permits.try_acquire_owned().unwrap(),
        oneshot::channel().1,
        &instructions,
    )
    .err()
    .expect("missing threadId");
    assert!(missing_thread.to_string().contains("missing threadId"));
}

#[tokio::test]
async fn replacement_settles_successful_failed_and_dropped_cancellations() {
    for result in [Ok(()), Err(anyhow!("cancel failed"))] {
        let active_turns = ActiveTurns::default();
        let (cancel, cancel_receiver) = oneshot::channel();
        active_turns
            .borrow_mut()
            .insert("session".to_owned(), Some(cancel));
        let settle = settle_replacement(cancel_receiver, Rc::clone(&active_turns), Some(result));
        let replace = replace_active_turn(AcpProvider::Grok, &active_turns, "session");
        let (replace, ()) = tokio::join!(replace, settle);
        assert!(replace.is_ok());
    }

    let active_turns = ActiveTurns::default();
    let (cancel, cancel_receiver) = oneshot::channel();
    active_turns
        .borrow_mut()
        .insert("session".to_owned(), Some(cancel));
    let settle = settle_replacement(cancel_receiver, Rc::clone(&active_turns), None);
    let replace = replace_active_turn(AcpProvider::Grok, &active_turns, "session");
    let (replace, ()) = tokio::join!(replace, settle);
    assert!(replace.is_ok());
}

async fn settle_replacement(
    cancel_receiver: oneshot::Receiver<CancelRequest>,
    active_turns: ActiveTurns,
    result: Option<Result<()>>,
) {
    let request = cancel_receiver.await.unwrap();
    if let Some(result) = result {
        assert!(request.response.send(result).is_ok());
    } else {
        drop(request.response);
    }
    active_turns.borrow_mut().remove("session");
}

#[tokio::test]
async fn allocates_turn_permits_for_user_and_background_work() {
    let shared = Arc::new(tokio::sync::Semaphore::new(1));
    let outer = Arc::new(tokio::sync::Semaphore::new(1));
    let background = acquire_turn_permit(&shared, &outer, false).await.unwrap();
    assert_eq!(shared.available_permits(), 0);
    drop(background);

    let shared_hold = Arc::clone(&shared).acquire_owned().await.unwrap();
    let user = acquire_turn_permit(&shared, &outer, true).await.unwrap();
    assert_eq!(outer.available_permits(), 0);
    drop((shared_hold, user));

    shared.close();
    outer.close();
    assert!(acquire_turn_permit(&shared, &outer, true).await.is_err());
    assert!(acquire_turn_permit(&shared, &outer, false).await.is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn driver_executes_queued_cancellation_and_emits_a_terminal_event() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let events = Arc::new(ThreadEventDispatcher::default());
            let receiver = events.subscribe("session");
            let active_turns = ActiveTurns::default();
            let invalidated_sessions = InvalidatedSessions::default();
            let permits = Arc::new(tokio::sync::Semaphore::new(1));
            let (cancellation, cancellation_receiver) = oneshot::channel();
            let (response, result) = oneshot::channel();
            assert!(cancellation.send(CancelRequest { response }).is_ok());
            let (turns, receiver_turns) = mpsc::channel(1);
            let worker = tokio::task::spawn_local(drive_turns(
                TurnDriver {
                    provider: AcpProvider::Grok,
                    connection: Rc::new(disconnected_connection(Arc::clone(&events))),
                    model: "model".to_owned(),
                    events: Arc::clone(&events),
                    active_turns,
                    invalidated_sessions,
                    alive: Arc::new(AtomicBool::new(true)),
                    cooldown: Arc::new(AtomicBool::new(false)),
                },
                receiver_turns,
            ));
            turns
                .send(PreparedTurn {
                    session_id: "session".to_owned(),
                    model: "model".to_owned(),
                    prompt: "unused".to_owned(),
                    effort: None,
                    cancellation: cancellation_receiver,
                    _permit: permits.acquire_owned().await.unwrap(),
                })
                .await
                .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_secs(1), result)
                    .await
                    .expect("queued cancellation was not executed")
                    .unwrap()
                    .is_ok()
            );
            assert_eq!(
                receiver.recv().await.unwrap()["params"]["turn"]["status"],
                "cancelled"
            );
            drop(turns);
            worker.await.unwrap();
        })
        .await;
}

fn disconnected_connection(events: Arc<ThreadEventDispatcher>) -> acp::ClientSideConnection {
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
