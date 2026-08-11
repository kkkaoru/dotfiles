use std::{future::Future, rc::Rc, sync::Arc};

use anyhow::{Result, anyhow};
use serde_json::json;
use tokio::sync::mpsc;

use super::{
    PreparedTurn, TurnDriver,
    execute::{TurnExecution, execute_turn},
};
use crate::app_server::events::ThreadEventDispatcher;

pub(in crate::grok_acp) async fn drive_turns(
    driver: TurnDriver,
    turns: mpsc::Receiver<PreparedTurn>,
) {
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

pub(in crate::grok_acp) async fn drive_turn_tasks<F, Fut>(
    mut turns: mpsc::Receiver<PreparedTurn>,
    mut start: F,
) where
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

pub(in crate::grok_acp) fn dispatch_turn_terminal(
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
pub(in crate::grok_acp) async fn acquire_turn_permit(
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
