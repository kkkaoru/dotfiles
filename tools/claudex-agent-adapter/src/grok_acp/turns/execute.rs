use std::{cell::Cell, future::Future, rc::Rc, sync::atomic::AtomicBool, time::Duration};

use agent_client_protocol::{self as acp, Agent as _};
use anyhow::anyhow;
use tokio::sync::oneshot;

use super::{
    ActiveTurns, CancelRequest, InvalidatedSessions, PreparedTurn, cancellation::CancelCtx,
    cancellation::cancel_prompt, cancellation::cancel_setup,
    cancellation::finish_setup_cancellation, configured_prompt,
};
use crate::{
    app_server::events::ThreadEventDispatcher,
    grok_acp::{connection::AcpProvider, updates},
};

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

mod setup;
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

fn finish_effort_setup(ctl: &mut TurnCtl<'_>, setup_result: Result<(), EffortSetupError>) -> bool {
    match setup_result {
        Ok(()) => {
            if let Ok(cancellation) = ctl.cancellation.try_recv() {
                ctl.finish_pre_prompt_cancel(cancellation);
                return false;
            }
            true
        }
        Err(EffortSetupError::TimedOut) if ctl.provider.is_session_scoped_configured() => {
            fail_model_setup(
                ctl,
                format!(
                    "{} ACP model selection timed out after {:?}",
                    ctl.provider.label(),
                    EFFORT_SETUP_TIMEOUT
                ),
            )
        }
        Err(EffortSetupError::TimedOut) => continue_without_effort(
            ctl,
            format!(
                "{} ACP set effort timed out after {:?}; continuing with provider default",
                ctl.provider.label(),
                EFFORT_SETUP_TIMEOUT
            ),
        ),
        Err(EffortSetupError::Failed(error)) if ctl.provider.model_is_launch_scoped() => {
            continue_without_effort(
                ctl,
                format!(
                    "{} ACP set effort failed ({error:?}); continuing with provider default",
                    ctl.provider.label()
                ),
            )
        }
        Err(EffortSetupError::Failed(error)) => fail_model_setup(
            ctl,
            format!(
                "{} ACP model selection failed: {error:?}",
                ctl.provider.label()
            ),
        ),
    }
}

fn fail_model_setup(ctl: &mut TurnCtl<'_>, message: String) -> bool {
    drop(ctl.permit.take());
    ctl.active_turns.borrow_mut().remove(ctl.session_id);
    if let Ok(cancellation) = ctl.cancellation.try_recv() {
        let _ = cancellation.response.send(Err(anyhow!(message.clone())));
    }
    updates::dispatch_error(ctl.events, ctl.session_id, message);
    false
}

fn continue_without_effort(ctl: &mut TurnCtl<'_>, warning: String) -> bool {
    tracing::warn!(session_id = ctl.session_id, "{warning}");
    if let Ok(cancellation) = ctl.cancellation.try_recv() {
        ctl.finish_pre_prompt_cancel(cancellation);
        return false;
    }
    true
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

async fn run_prompt(
    mut ctl: TurnCtl<'_>,
    connection: Rc<acp::ClientSideConnection>,
    id: acp::SessionId,
    prompt: String,
    timeout: Duration,
    alive: &AtomicBool,
) {
    let request = acp::PromptRequest::new(
        id.clone(),
        vec![acp::ContentBlock::Text(acp::TextContent::new(prompt))],
    );
    let response = match configured_prompt::wait(
        ctl.provider,
        timeout,
        prompt_once(&mut ctl, &connection, request),
    )
    .await
    {
        configured_prompt::Wait::Completed(Some(response)) => response,
        configured_prompt::Wait::Completed(None) => return,
        configured_prompt::Wait::TimedOut => {
            let message = format!(
                "{} ACP prompt timed out after {:?}; recycling provider",
                ctl.provider.label(),
                timeout
            );
            configured_prompt::invalidate(
                ctl.provider,
                configured_prompt::Invalidation {
                    session_id: ctl.session_id,
                    permit: &mut *ctl.permit,
                    events: ctl.events,
                    active_turns: ctl.active_turns,
                    invalidated_sessions: ctl.invalidated_sessions,
                    alive,
                    message,
                },
            );
            return;
        }
    };
    let is_session_configured = ctl.provider.is_session_scoped_configured();
    if let (true, Err(error)) = (is_session_configured, response.as_ref()) {
        let message = format!(
            "{} ACP prompt failed: {error:?}; recycling provider",
            ctl.provider.label()
        );
        configured_prompt::invalidate(
            ctl.provider,
            configured_prompt::Invalidation {
                session_id: ctl.session_id,
                permit: &mut *ctl.permit,
                events: ctl.events,
                active_turns: ctl.active_turns,
                invalidated_sessions: ctl.invalidated_sessions,
                alive,
                message,
            },
        );
        return;
    }
    drop(ctl.permit.take());
    ctl.active_turns.borrow_mut().remove(ctl.session_id);
    configured_prompt::finish(ctl.provider, ctl.session_id, response, ctl.events).await;
}

async fn prompt_once(
    ctl: &mut TurnCtl<'_>,
    connection: &Rc<acp::ClientSideConnection>,
    request: acp::PromptRequest,
) -> Option<acp::Result<acp::PromptResponse>> {
    let session_id = ctl.session_id;
    let prompt_started = Rc::new(Cell::new(false));
    let prompt = {
        let connection = Rc::clone(connection);
        let prompt_started = Rc::clone(&prompt_started);
        async move {
            prompt_started.set(true);
            connection.prompt(request).await
        }
    };
    tokio::pin!(prompt);
    let response = tokio::select! {
        biased;
        response = &mut prompt => {
            if let Ok(cancellation) = ctl.cancellation.try_recv() {
                tracing::debug!(
                    session_id,
                    "ACP prompt completion won the session cancellation race"
                );
                let _ = cancellation.response.send(Ok(()));
            }
            Some(response)
        }
        cancellation = &mut *ctl.cancellation => match cancellation {
            Ok(cancellation) => {
                handle_prompt_cancellation(
                    ctl,
                    connection,
                    prompt_started.get(),
                    prompt,
                    cancellation,
                )
                .await;
                None
            }
            Err(_) => Some(prompt.await),
        },
    };
    response
}

async fn handle_prompt_cancellation<F>(
    ctl: &mut TurnCtl<'_>,
    connection: &acp::ClientSideConnection,
    prompt_started: bool,
    prompt: F,
    cancellation: CancelRequest,
) where
    F: Future<Output = acp::Result<acp::PromptResponse>>,
{
    if prompt_started {
        let permit = ctl.take_permit();
        cancel_prompt(ctl.cancel_ctx(permit, cancellation), connection, prompt).await;
        return;
    }
    ctl.finish_pre_prompt_cancel(cancellation);
}

#[cfg(test)]
include!("execute_tests.rs");
