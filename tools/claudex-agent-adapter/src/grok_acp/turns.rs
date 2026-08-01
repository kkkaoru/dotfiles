use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    future::Future,
    rc::Rc,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use agent_client_protocol as acp;
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, timeout};

use super::{connection::AcpProvider, prompt};
use crate::app_server::events::ThreadEventDispatcher;

mod cancellation;
mod configured_prompt;
mod execute;

use execute::{TurnExecution, execute_turn};

/// How long a same-session replace waits for the prior turn to leave `active_turns`.
const REPLACE_SETTLE_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) struct PreparedTurn {
    pub(super) session_id: String,
    pub(super) prompt: String,
    pub(super) effort: Option<String>,
    pub(super) cancellation: oneshot::Receiver<CancelRequest>,
    pub(super) _permit: tokio::sync::OwnedSemaphorePermit,
}

pub(super) struct CancelRequest {
    pub(super) response: oneshot::Sender<Result<()>>,
}

pub(super) type ActiveTurns = Rc<RefCell<HashMap<String, Option<oneshot::Sender<CancelRequest>>>>>;
pub(super) type InvalidatedSessions = Rc<RefCell<HashSet<String>>>;

pub(super) async fn queue_turn(
    provider: AcpProvider,
    params: Value,
    permit: tokio::sync::OwnedSemaphorePermit,
    instructions: &Rc<RefCell<HashMap<String, String>>>,
    turns: &mpsc::Sender<PreparedTurn>,
    active_turns: &ActiveTurns,
    invalidated_sessions: &InvalidatedSessions,
) -> Result<()> {
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let turn = prepare_turn(provider, params, permit, cancel_rx, instructions)?;
    if invalidated_sessions.borrow().contains(&turn.session_id) {
        return Err(anyhow!(
            "{} ACP session `{}` was invalidated after cancellation failed to settle",
            provider.label(),
            turn.session_id
        ));
    }
    // Same-session follow-ups must replace the in-flight turn instead of failing
    // with "already has an active turn" (that error deferred live user messages).
    if active_turns.borrow().contains_key(&turn.session_id) {
        replace_active_turn(provider, active_turns, &turn.session_id).await?;
    }
    let session_id = turn.session_id.clone();
    active_turns
        .borrow_mut()
        .insert(session_id.clone(), Some(cancel_tx));
    if turns.send(turn).await.is_err() {
        active_turns.borrow_mut().remove(&session_id);
        return Err(anyhow!("ACP turn worker is unavailable"));
    }
    Ok(())
}

async fn replace_active_turn(
    provider: AcpProvider,
    active_turns: &ActiveTurns,
    session_id: &str,
) -> Result<()> {
    tracing::info!(
        session_id,
        provider = provider.label(),
        "replacing in-flight ACP turn for a newer request on the same session"
    );
    let (response_tx, response_rx) = oneshot::channel();
    cancel_turn(active_turns, session_id, response_tx);
    match timeout(REPLACE_SETTLE_TIMEOUT, response_rx).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => {
            tracing::warn!(
                %error,
                session_id,
                "cancel during same-session replace returned an error; waiting for worker exit"
            );
        }
        Ok(Err(_)) => {
            tracing::warn!(
                session_id,
                "cancel response dropped during same-session replace"
            );
        }
        Err(_) => {
            tracing::warn!(
                session_id,
                "cancel did not settle within {:?}; waiting for active_turns clear",
                REPLACE_SETTLE_TIMEOUT
            );
        }
    }
    let deadline = tokio::time::Instant::now() + REPLACE_SETTLE_TIMEOUT;
    while active_turns.borrow().contains_key(session_id) {
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "{} ACP session `{}` still has an active turn after replace cancel",
                provider.label(),
                session_id
            ));
        }
        // Yield so the turn worker on this LocalSet can finish execute_turn.
        tokio::task::yield_now().await;
        sleep(Duration::from_millis(10)).await;
    }
    Ok(())
}

pub(super) fn cancel_turn(
    active_turns: &ActiveTurns,
    session_id: &str,
    response: oneshot::Sender<Result<()>>,
) {
    let cancellation = match take_cancellation(active_turns, session_id) {
        Ok(Some(cancellation)) => cancellation,
        Ok(None) => {
            let _ = response.send(Ok(()));
            return;
        }
        Err(error) => {
            let _ = response.send(Err(error));
            return;
        }
    };
    if let Err(request) = cancellation.send(CancelRequest { response }) {
        let _ = request.response.send(Ok(()));
    }
}

fn take_cancellation(
    active_turns: &ActiveTurns,
    session_id: &str,
) -> Result<Option<oneshot::Sender<CancelRequest>>> {
    let mut active_turns = active_turns.borrow_mut();
    let Some(cancellation) = active_turns.get_mut(session_id) else {
        return Ok(None);
    };
    cancellation
        .take()
        .map(Some)
        .ok_or_else(|| anyhow!("ACP session `{session_id}` cancellation is already in progress"))
}

fn prepare_turn(
    provider: AcpProvider,
    params: Value,
    permit: tokio::sync::OwnedSemaphorePermit,
    cancellation: oneshot::Receiver<CancelRequest>,
    instructions: &Rc<RefCell<HashMap<String, String>>>,
) -> Result<PreparedTurn> {
    let session_id = params
        .get("threadId")
        .and_then(Value::as_str)
        .with_context(|| format!("{} ACP turn is missing threadId", provider.label()))?
        .to_owned();
    let prompt = prompt::input_text(params.get("input").unwrap_or(&Value::Null));
    let prefix = instructions.borrow_mut().remove(&session_id);
    let prompt = match prefix {
        Some(prefix) => format!("{prefix}\n\n{prompt}"),
        None => prompt,
    };
    let effort = params
        .get("effort")
        .and_then(Value::as_str)
        .and_then(|effort| match provider {
            AcpProvider::Grok => None,
            AcpProvider::Configured
            | AcpProvider::ConfiguredLaunchScoped
            | AcpProvider::Copilot => prompt::copilot_effort(effort),
        })
        .map(str::to_owned);
    Ok(PreparedTurn {
        session_id,
        prompt,
        effort,
        cancellation,
        _permit: permit,
    })
}

pub(super) struct TurnDriver {
    pub(super) provider: AcpProvider,
    pub(super) connection: Rc<acp::ClientSideConnection>,
    pub(super) model: String,
    pub(super) events: Arc<ThreadEventDispatcher>,
    pub(super) active_turns: ActiveTurns,
    pub(super) invalidated_sessions: InvalidatedSessions,
    pub(super) alive: Arc<AtomicBool>,
}

pub(super) async fn drive_turns(driver: TurnDriver, turns: mpsc::Receiver<PreparedTurn>) {
    let TurnDriver {
        provider,
        connection,
        model,
        events,
        active_turns,
        invalidated_sessions,
        alive,
    } = driver;
    drive_turn_tasks(turns, move |turn| {
        let connection = Rc::clone(&connection);
        let model = model.clone();
        let events = Arc::clone(&events);
        let active_turns = Rc::clone(&active_turns);
        let invalidated_sessions = Rc::clone(&invalidated_sessions);
        let alive = Arc::clone(&alive);
        async move {
            let session_id = turn.session_id.clone();
            execute_turn(
                TurnExecution {
                    provider,
                    connection,
                    model: &model,
                    events: &events,
                    active_turns: &active_turns,
                    invalidated_sessions: &invalidated_sessions,
                    alive: &alive,
                },
                turn,
            )
            .await;
            active_turns.borrow_mut().remove(&session_id);
        }
    })
    .await;
}

pub(super) async fn drive_turn_tasks<F, Fut>(mut turns: mpsc::Receiver<PreparedTurn>, mut start: F)
where
    F: FnMut(PreparedTurn) -> Fut,
    Fut: Future<Output = ()> + 'static,
{
    let mut active = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            turn = turns.recv() => match turn {
                Some(turn) => {
                    active.spawn_local(start(turn));
                }
                None => break,
            },
            completed = active.join_next(), if !active.is_empty() => {
                if let Some(Err(error)) = completed {
                    tracing::error!(?error, "ACP turn task stopped unexpectedly");
                }
            }
        }
    }
    active.abort_all();
    while active.join_next().await.is_some() {}
}

pub(super) fn dispatch_turn_terminal(
    events: &ThreadEventDispatcher,
    session_id: &str,
    status: &str,
) {
    events.dispatch(json!({
        "method":"turn/completed",
        "params":{"threadId":session_id,"turn":{"status":status}}
    }));
}

/// User turns wait on outer reserve **or** shared pool so SubAgents cannot starve them.
pub(super) async fn acquire_turn_permit(
    shared: &Arc<tokio::sync::Semaphore>,
    outer: &Arc<tokio::sync::Semaphore>,
    is_user: bool,
) -> Result<tokio::sync::OwnedSemaphorePermit> {
    if !is_user {
        return Arc::clone(shared)
            .acquire_owned()
            .await
            .map_err(|_| anyhow!("ACP driver is unavailable"));
    }
    tokio::select! {
        permit = Arc::clone(outer).acquire_owned() => {
            permit.map_err(|_| anyhow!("ACP driver is unavailable"))
        }
        permit = Arc::clone(shared).acquire_owned() => {
            permit.map_err(|_| anyhow!("ACP driver is unavailable"))
        }
    }
}

#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "turns_tests.rs"]
mod tests;
