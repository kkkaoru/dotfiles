use std::{future::Future, rc::Rc, sync::atomic::AtomicBool, time::Duration};

use agent_client_protocol as acp;
use tokio::sync::oneshot;

use super::{
    ActiveTurns, CancelRequest, InvalidatedSessions, PreparedTurn, cancellation::CancelCtx,
    cancellation::cancel_setup, cancellation::finish_setup_cancellation, configured_prompt,
};
use crate::{app_server::events::ThreadEventDispatcher, grok_acp::connection::AcpProvider};

// Session creation is bounded; effort setup must not hang a turn forever when a
// provider ignores or stalls on set_session_model (observed with configured ACP).
const EFFORT_SETUP_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) enum EffortSetupError {
    TimedOut,
    Failed(acp::Error),
}

pub(super) struct TurnCtl<'a> {
    provider: AcpProvider,
    session_id: &'a str,
    cancellation: &'a mut oneshot::Receiver<CancelRequest>,
    permit: &'a mut Option<tokio::sync::OwnedSemaphorePermit>,
    events: &'a ThreadEventDispatcher,
    active_turns: &'a ActiveTurns,
    invalidated_sessions: &'a InvalidatedSessions,
}

mod effort;
mod setup;
#[path = "execute_prompt.rs"]
mod prompt;
use prompt::run_prompt;
#[cfg(test)]
use prompt::handle_prompt_cancellation;
use effort::finish_effort_setup;
use setup::apply_effort;

pub(super) struct TurnExecution<'a> {
    pub(super) provider: AcpProvider,
    pub(super) connection: Rc<acp::ClientSideConnection>,
    pub(super) model: &'a str,
    pub(super) events: &'a ThreadEventDispatcher,
    pub(super) active_turns: &'a ActiveTurns,
    pub(super) invalidated_sessions: &'a InvalidatedSessions,
    pub(super) alive: &'a AtomicBool,
}

impl TurnCtl<'_> {
    fn take_permit(&mut self) -> tokio::sync::OwnedSemaphorePermit {
        self.permit.take().expect("active turn permit")
    }

    fn cancel_ctx(
        &self,
        permit: tokio::sync::OwnedSemaphorePermit,
        cancellation: CancelRequest,
    ) -> CancelCtx<'_> {
        CancelCtx {
            provider: self.provider,
            session_id: self.session_id,
            permit,
            cancellation,
            events: self.events,
            invalidated_sessions: self.invalidated_sessions,
        }
    }

    fn finish_pre_prompt_cancel(&mut self, cancellation: CancelRequest) {
        finish_setup_cancellation(
            self.session_id,
            self.take_permit(),
            cancellation,
            self.events,
            self.active_turns,
        );
    }
}

pub(super) async fn execute_turn(context: TurnExecution<'_>, turn: PreparedTurn) {
    let TurnExecution {
        provider,
        connection,
        model,
        events,
        active_turns,
        invalidated_sessions,
        alive,
    } = context;
    let PreparedTurn {
        session_id,
        prompt,
        effort,
        mut cancellation,
        _permit: permit,
    } = turn;
    let mut permit = Some(permit);
    let mut ctl = TurnCtl {
        provider,
        session_id: &session_id,
        cancellation: &mut cancellation,
        permit: &mut permit,
        events,
        active_turns,
        invalidated_sessions,
    };
    let id = acp::SessionId::new(session_id.clone());
    if let Ok(cancellation) = ctl.cancellation.try_recv() {
        ctl.finish_pre_prompt_cancel(cancellation);
        return;
    }
    if !apply_effort(&mut ctl, &connection, model, effort.as_deref(), &id).await {
        return;
    }
    let timeout = configured_prompt::TIMEOUT;
    run_prompt(ctl, connection, id, prompt, timeout, alive).await;
}

pub(super) async fn handle_setup_cancellation<F, T>(
    ctl: &mut TurnCtl<'_>,
    setup_started: bool,
    setup: F,
    cancellation: CancelRequest,
) where
    F: Future<Output = T>,
{
    let permit = ctl.take_permit();
    if setup_started {
        let active_turns = ctl.active_turns;
        cancel_setup(ctl.cancel_ctx(permit, cancellation), active_turns, setup).await;
        return;
    }
    finish_setup_cancellation(
        ctl.session_id,
        permit,
        cancellation,
        ctl.events,
        ctl.active_turns,
    );
}


#[cfg(test)]
include!("execute_tests.rs");
