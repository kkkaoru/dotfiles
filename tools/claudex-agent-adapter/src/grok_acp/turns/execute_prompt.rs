use std::{
    cell::Cell,
    future::Future,
    rc::Rc,
    sync::atomic::AtomicBool,
    time::Duration,
};

use agent_client_protocol::{self as acp, Agent as _};
use tokio::sync::watch;

use super::super::{CancelRequest, cancellation::cancel_prompt, configured_prompt};
use super::TurnCtl;

pub(super) struct PromptGuard<'a> {
    pub(super) timeout: Duration,
    pub(super) alive: &'a AtomicBool,
    pub(super) cooldown: &'a AtomicBool,
    pub(super) quota: Option<watch::Receiver<Option<String>>>,
}

pub(super) async fn run_prompt(
    mut ctl: TurnCtl<'_>,
    connection: Rc<acp::ClientSideConnection>,
    id: acp::SessionId,
    prompt: String,
    mut guard: PromptGuard<'_>,
) {
    let request = acp::PromptRequest::new(
        id.clone(),
        vec![acp::ContentBlock::Text(acp::TextContent::new(prompt.clone()))],
    );
    let activity = ctl.events.subscribe(ctl.session_id);
    let saw_activity = AtomicBool::new(false);
    let response = match configured_prompt::wait_with_activity(
        ctl.provider,
        guard.timeout,
        prompt_once(&mut ctl, &connection, request),
        Some(activity),
        guard.quota.as_mut(),
        Some(&saw_activity),
    )
    .await
    {
        configured_prompt::Wait::Completed(Some(response)) => response,
        configured_prompt::Wait::Completed(None) => return,
        configured_prompt::Wait::TimedOut => {
            let message = timeout_message(&ctl, &guard);
            fail_prompt(&mut ctl, &connection, &guard, message, true).await;
            return;
        }
        configured_prompt::Wait::Quota(message) => {
            let message = quota_message(&ctl, &message);
            fail_prompt(&mut ctl, &connection, &guard, message, true).await;
            return;
        }
    };
    let response = retry_unknown_session_prompt(
        &mut ctl,
        &connection,
        &prompt,
        response,
        saw_activity.load(std::sync::atomic::Ordering::Acquire),
    )
    .await;
    finish_prompt_result(ctl, response, &guard).await;
}

async fn retry_unknown_session_prompt(
    ctl: &mut TurnCtl<'_>,
    connection: &Rc<acp::ClientSideConnection>,
    prompt: &str,
    first: acp::Result<acp::PromptResponse>,
    saw_activity: bool,
) -> acp::Result<acp::PromptResponse> {
    super::unknown_session::retry_unknown_session_once(first, saw_activity, async {
        recreate_session_and_prompt(ctl, connection, prompt).await
    })
    .await
    .0
}

async fn recreate_session_and_prompt(
    _ctl: &mut TurnCtl<'_>,
    connection: &Rc<acp::ClientSideConnection>,
    prompt: &str,
) -> acp::Result<acp::PromptResponse> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let created = connection
        .new_session(acp::NewSessionRequest::new(cwd))
        .await?;
    let request = acp::PromptRequest::new(
        created.session_id,
        vec![acp::ContentBlock::Text(acp::TextContent::new(
            prompt.to_owned(),
        ))],
    );
    connection.prompt(request).await
}

fn timeout_message(ctl: &TurnCtl<'_>, guard: &PromptGuard<'_>) -> String {
    format!(
        "{} ACP prompt had no event for {:?}; provider/model cooling down",
        ctl.provider.label(),
        guard.timeout
    )
}

fn quota_message(ctl: &TurnCtl<'_>, message: &str) -> String {
    format!(
        "{} ACP quota exhausted: {message}; provider/model cooling down",
        ctl.provider.label()
    )
}

async fn fail_prompt(
    ctl: &mut TurnCtl<'_>,
    connection: &acp::ClientSideConnection,
    guard: &PromptGuard<'_>,
    message: String,
    cancel: bool,
) {
    if cancel {
        configured_prompt::cancel_timed_out_prompt(ctl.provider, connection, ctl.session_id).await;
    }
    configured_prompt::invalidate(
        ctl.provider,
        configured_prompt::Invalidation {
            session_id: ctl.session_id,
            permit: &mut *ctl.permit,
            events: ctl.events,
            active_turns: ctl.active_turns,
            invalidated_sessions: ctl.invalidated_sessions,
            alive: guard.alive,
            cooldown: guard.cooldown,
            trip_cooldown: true,
            message,
        },
    );
}

async fn finish_prompt_result(
    ctl: TurnCtl<'_>,
    response: acp::Result<acp::PromptResponse>,
    guard: &PromptGuard<'_>,
) {
    let is_session_configured = ctl.provider.is_session_scoped_configured();
    if let (true, Err(error)) = (is_session_configured, response.as_ref()) {
        let message = format!(
            "{}; recycling provider",
            configured_prompt::prompt_failure_message(ctl.provider, error)
        );
        configured_prompt::invalidate(
            ctl.provider,
            configured_prompt::Invalidation {
                session_id: ctl.session_id,
                permit: &mut *ctl.permit,
                events: ctl.events,
                active_turns: ctl.active_turns,
                invalidated_sessions: ctl.invalidated_sessions,
                alive: guard.alive,
                cooldown: guard.cooldown,
                trip_cooldown: false,
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
