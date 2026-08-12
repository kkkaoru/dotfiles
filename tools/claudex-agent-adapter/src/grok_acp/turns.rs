use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use agent_client_protocol as acp;
use anyhow::{Result, anyhow};
use serde_json::Value;
#[allow(unused_imports)]
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use super::connection::AcpProvider;
use crate::app_server::events::ThreadEventDispatcher;

mod cancellation;
mod configured_prompt;
mod execute;

/// How long a same-session replace waits for the prior turn to leave `active_turns`.
/// Bound tightly so mid-turn user steering does not stall the Claude Code UI.
pub(super) const REPLACE_SETTLE_TIMEOUT: Duration = Duration::from_millis(200);

pub(super) struct PreparedTurn {
    pub(super) session_id: String,
    /// The model selected by this routed request. A session-scoped configured
    /// ACP child may serve several model routes, so this must not be borrowed
    /// from the first route that happened to start the process.
    pub(super) model: String,
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

#[path = "turns_replace.rs"]
mod replace;
use replace::replace_active_turn;

mod prepare;
use prepare::{prepare_turn, take_cancellation};

pub(super) struct TurnDriver {
    pub(super) provider: AcpProvider,
    pub(super) connection: Rc<acp::ClientSideConnection>,
    pub(super) model: String,
    pub(super) events: Arc<ThreadEventDispatcher>,
    pub(super) active_turns: ActiveTurns,
    pub(super) invalidated_sessions: InvalidatedSessions,
    pub(super) alive: Arc<AtomicBool>,
    pub(super) cooldown: Arc<AtomicBool>,
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
