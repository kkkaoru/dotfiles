use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    future::Future,
    rc::Rc,
    sync::Arc,
};

use agent_client_protocol as acp;
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use super::{connection::AcpProvider, prompt};
use crate::app_server::events::ThreadEventDispatcher;

mod cancellation;
mod execute;

use execute::execute_turn;

pub(super) struct PreparedTurn {
    pub(super) session_id: String,
    pub(super) prompt: String,
    pub(super) effort: Option<String>,
    pub(super) cancellation: oneshot::Receiver<CancelRequest>,
    pub(super) _permit: tokio::sync::OwnedSemaphorePermit,
}

pub(super) struct CancelRequest {
    response: oneshot::Sender<Result<()>>,
}

pub(super) type ActiveTurns =
    Rc<RefCell<HashMap<String, Option<oneshot::Sender<CancelRequest>>>>>;
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
    if active_turns.borrow().contains_key(&turn.session_id) {
        return Err(anyhow!(
            "{} ACP session already has an active turn",
            provider.label()
        ));
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

pub(super) fn cancel_turn(
    active_turns: &ActiveTurns,
    session_id: &str,
    response: oneshot::Sender<Result<()>>,
) {
    let cancellation = {
        let mut active_turns = active_turns.borrow_mut();
        match active_turns.get_mut(session_id) {
            Some(cancellation) => {
                let Some(cancellation) = cancellation.take() else {
                    let _ = response.send(Err(anyhow!(
                        "ACP session `{session_id}` cancellation is already in progress"
                    )));
                    return;
                };
                cancellation
            }
            None => {
                let _ = response.send(Ok(()));
                return;
            }
        }
    };
    if let Err(request) = cancellation.send(CancelRequest { response }) {
        let _ = request.response.send(Ok(()));
    }
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
            AcpProvider::Grok => prompt::grok_effort(effort),
            AcpProvider::Copilot => prompt::copilot_effort(effort),
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

pub(super) async fn drive_turns(
    provider: AcpProvider,
    connection: Rc<acp::ClientSideConnection>,
    model: String,
    turns: mpsc::Receiver<PreparedTurn>,
    events: Arc<ThreadEventDispatcher>,
    active_turns: ActiveTurns,
    invalidated_sessions: InvalidatedSessions,
) {
    drive_turn_tasks(turns, move |turn| {
        let connection = Rc::clone(&connection);
        let model = model.clone();
        let events = Arc::clone(&events);
        let active_turns = Rc::clone(&active_turns);
        let invalidated_sessions = Rc::clone(&invalidated_sessions);
        async move {
            let session_id = turn.session_id.clone();
            execute_turn(
                provider,
                connection,
                &model,
                turn,
                &events,
                &active_turns,
                &invalidated_sessions,
            )
            .await;
            active_turns.borrow_mut().remove(&session_id);
        }
    })
    .await;
}

pub(super) async fn drive_turn_tasks<F, Fut>(
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
