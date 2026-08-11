use std::{cell::Cell, future::Future, rc::Rc, sync::atomic::AtomicBool, time::Duration};

use agent_client_protocol::{self as acp, Agent as _};

use super::super::{CancelRequest, cancellation::cancel_prompt, configured_prompt};
use super::TurnCtl;

pub(super) async fn run_prompt(
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

pub(super) async fn handle_prompt_cancellation<F>(
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
