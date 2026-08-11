use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use agent_client_protocol as acp;
use anyhow::{Context, Result, anyhow};
use serde_json::Value;
#[allow(unused_imports)]
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep, timeout_at};

use super::{connection::AcpProvider, prompt};
use crate::app_server::events::ThreadEventDispatcher;

mod cancellation;
mod configured_prompt;
mod execute;

/// How long a same-session replace waits for the prior turn to leave `active_turns`.
/// Bound tightly so mid-turn user steering does not stall the Claude Code UI.
pub(super) const REPLACE_SETTLE_TIMEOUT: Duration = Duration::from_millis(200);

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
    // One shared budget for cancel ack + active_turns clear. Stacking two
    // REPLACE_SETTLE_TIMEOUT waits doubled mid-turn steering latency.
    let deadline = Instant::now() + REPLACE_SETTLE_TIMEOUT;
    let (response_tx, response_rx) = oneshot::channel();
    cancel_turn(active_turns, session_id, response_tx);
    match timeout_at(deadline, response_rx).await {
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
    while active_turns.borrow().contains_key(session_id) {
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "{} ACP session `{}` still has an active turn after replace cancel",
                provider.label(),
                session_id
            ));
        }
        // Yield so the turn worker on this LocalSet can finish execute_turn.
        tokio::task::yield_now().await;
        sleep(Duration::from_millis(1)).await;
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

mod drive;
#[cfg(test)]
pub(super) use drive::drive_turn_tasks;
pub(super) use drive::{acquire_turn_permit, dispatch_turn_terminal, drive_turns};

#[cfg(test)]
// Coverage excludes test implementation; production behavior remains measured.
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "turns_tests.rs"]
mod tests;
