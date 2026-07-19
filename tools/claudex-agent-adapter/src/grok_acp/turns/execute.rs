use std::{cell::Cell, future::Future, rc::Rc};

use agent_client_protocol::{self as acp, Agent as _};
use anyhow::anyhow;
use serde_json::{Map, Value};
use tokio::sync::oneshot;

use super::{
    ActiveTurns, CancelRequest, InvalidatedSessions, PreparedTurn, cancellation::CancelCtx,
    cancellation::cancel_prompt, cancellation::cancel_setup,
    cancellation::finish_setup_cancellation, dispatch_turn_terminal,
};
use crate::{
    app_server::events::ThreadEventDispatcher,
    grok_acp::{connection::AcpProvider, updates},
};

struct TurnCtl<'a> {
    provider: AcpProvider,
    session_id: &'a str,
    cancellation: &'a mut oneshot::Receiver<CancelRequest>,
    permit: &'a mut Option<tokio::sync::OwnedSemaphorePermit>,
    events: &'a ThreadEventDispatcher,
    active_turns: &'a ActiveTurns,
    invalidated_sessions: &'a InvalidatedSessions,
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

pub(super) async fn execute_turn(
    provider: AcpProvider,
    connection: Rc<acp::ClientSideConnection>,
    model: &str,
    turn: PreparedTurn,
    events: &ThreadEventDispatcher,
    active_turns: &ActiveTurns,
    invalidated_sessions: &InvalidatedSessions,
) {
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
    run_prompt(ctl, connection, id, prompt).await;
}

async fn apply_effort(
    ctl: &mut TurnCtl<'_>,
    connection: &Rc<acp::ClientSideConnection>,
    model: &str,
    effort: Option<&str>,
    id: &acp::SessionId,
) -> bool {
    let Some(effort) = effort else {
        return true;
    };
    let mut meta = Map::new();
    meta.insert(
        "reasoningEffort".to_owned(),
        Value::String(effort.to_owned()),
    );
    let request =
        acp::SetSessionModelRequest::new(id.clone(), model.to_owned()).meta(Some(meta));
    let setup_started = Rc::new(Cell::new(false));
    let setup = {
        let connection = Rc::clone(connection);
        let setup_started = Rc::clone(&setup_started);
        async move {
            setup_started.set(true);
            connection.set_session_model(request).await
        }
    };
    tokio::pin!(setup);
    let setup_result = tokio::select! {
        biased;
        cancellation_result = &mut *ctl.cancellation => match cancellation_result {
            Ok(cancellation) => {
                handle_setup_cancellation(ctl, setup_started.get(), &mut setup, cancellation)
                    .await;
                return false;
            }
            Err(_) => setup.await,
        },
        result = &mut setup => result,
    };
    if let Err(error) = setup_result {
        let message = format!(
            "{} ACP set effort failed: {error:?}",
            ctl.provider.label()
        );
        drop(ctl.permit.take());
        ctl.active_turns.borrow_mut().remove(ctl.session_id);
        if let Ok(cancellation) = ctl.cancellation.try_recv() {
            let _ = cancellation.response.send(Err(anyhow!(message.clone())));
        }
        updates::dispatch_error(ctl.events, ctl.session_id, message);
        return false;
    }
    if let Ok(cancellation) = ctl.cancellation.try_recv() {
        ctl.finish_pre_prompt_cancel(cancellation);
        return false;
    }
    true
}

async fn handle_setup_cancellation<F, T>(
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
) {
    let session_id = ctl.session_id;
    let request = acp::PromptRequest::new(
        id,
        vec![acp::ContentBlock::Text(acp::TextContent::new(prompt))],
    );
    let prompt_started = Rc::new(Cell::new(false));
    let prompt = {
        let connection = Rc::clone(&connection);
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
                    &mut ctl,
                    &connection,
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
    if let Some(response) = response {
        drop(ctl.permit.take());
        ctl.active_turns.borrow_mut().remove(session_id);
        finish_prompt(ctl.provider, session_id, response, ctl.events).await;
    }
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
    let permit = ctl.take_permit();
    if prompt_started {
        cancel_prompt(ctl.cancel_ctx(permit, cancellation), connection, prompt).await;
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

async fn finish_prompt(
    provider: AcpProvider,
    session_id: &str,
    response: acp::Result<acp::PromptResponse>,
    events: &ThreadEventDispatcher,
) {
    match response {
        Ok(response) => {
            // ACP handlers are local tasks. Yield so notifications parsed before the
            // prompt response are dispatched before the terminal event.
            tokio::task::yield_now().await;
            let status = if response.stop_reason == acp::StopReason::Cancelled {
                "cancelled"
            } else {
                "completed"
            };
            dispatch_turn_terminal(events, session_id, status);
        }
        Err(error) => {
            updates::dispatch_error(
                events,
                session_id,
                format!("{} ACP prompt failed: {error:?}", provider.label()),
            );
        }
    }
}
