use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use agent_client_protocol::{self as acp, Agent as _};
use serde_json::{Map, Value};

use super::{
    EFFORT_SETUP_TIMEOUT, EffortSetupError, TurnCtl, finish_effort_setup, handle_setup_cancellation,
};
use crate::grok_acp::connection::AcpProvider;

thread_local! {
    static APPLIED_SESSION_EFFORT: RefCell<HashMap<String, String>> =
        RefCell::new(HashMap::new());
}

pub(super) async fn apply_effort(
    ctl: &mut TurnCtl<'_>,
    connection: &Rc<acp::ClientSideConnection>,
    model: &str,
    effort: Option<&str>,
    id: &acp::SessionId,
) -> bool {
    // Grok and other CLI-model ACP launches pin model (and often effort) at process start.
    // Session create also pins the ACP model id once. Re-running set_session_model every
    // turn reselects Cursor's auto router and adds multi-second RPC latency before prompts.
    if ctl.provider == AcpProvider::Grok || ctl.provider.model_is_launch_scoped() {
        tracing::info!(
            session_id = ctl.session_id,
            effort,
            provider = ctl.provider.label(),
            "skipping ACP set_session_model; model and effort are launch-scoped"
        );
        return true;
    }
    if ctl.invalidated_sessions.borrow().contains(ctl.session_id) {
        forget_applied_effort(ctl.session_id);
    }
    if effort.is_none() {
        tracing::info!(
            session_id = ctl.session_id,
            provider = ctl.provider.label(),
            "skipping ACP set_session_model; session/new already pinned the model"
        );
        return true;
    }
    if let Some(effort) = effort
        && effort_already_applied(ctl.session_id, effort)
    {
        tracing::info!(
            session_id = ctl.session_id,
            effort,
            provider = ctl.provider.label(),
            "skipping ACP effort setup; session already pinned"
        );
        return true;
    }
    let setup_started = Rc::new(Cell::new(false));
    let setup = setup_effort(
        Rc::clone(connection),
        ctl.provider,
        model,
        effort,
        id.clone(),
        Rc::clone(&setup_started),
    );
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
    if setup_result.is_ok() {
        if let Some(effort) = effort {
            remember_applied_effort(ctl.session_id, effort);
        }
        tokio::task::yield_now().await;
    }
    finish_effort_setup(ctl, setup_result)
}

fn effort_already_applied(session_id: &str, effort: &str) -> bool {
    APPLIED_SESSION_EFFORT
        .with(|applied| applied.borrow().get(session_id).map(String::as_str) == Some(effort))
}

fn remember_applied_effort(session_id: &str, effort: &str) {
    APPLIED_SESSION_EFFORT.with(|applied| {
        applied
            .borrow_mut()
            .insert(session_id.to_owned(), effort.to_owned());
    });
}

fn forget_applied_effort(session_id: &str) {
    APPLIED_SESSION_EFFORT.with(|applied| {
        applied.borrow_mut().remove(session_id);
    });
}

fn effort_option_rejected(error: &acp::Error) -> bool {
    matches!(
        error.code,
        acp::ErrorCode::MethodNotFound | acp::ErrorCode::InvalidParams
    )
}

async fn setup_effort(
    connection: Rc<acp::ClientSideConnection>,
    provider: AcpProvider,
    model: &str,
    effort: Option<&str>,
    id: acp::SessionId,
    setup_started: Rc<Cell<bool>>,
) -> Result<(), EffortSetupError> {
    setup_started.set(true);
    let session_model = crate::grok_acp::prompt::configured_acp_session_model(model);
    // Session-scoped configured ACP (OpenCode): session/new already pinned the model.
    // Only push effort when requested; do not reselect the model every turn.
    if provider.is_session_scoped_configured() {
        let Some(effort) = effort else {
            return Ok(());
        };
        return match set_effort_option(&connection, &id, effort).await {
            Ok(()) => Ok(()),
            Err(EffortSetupError::Failed(error)) if effort_option_rejected(&error) => {
                tracing::debug!(
                    ?error,
                    "configured ACP rejected session effort option; falling back to set_session_model meta"
                );
                set_model(
                    &connection,
                    acp::SetSessionModelRequest::new(id, session_model)
                        .meta(model_meta(Some(effort))),
                )
                .await
            }
            Err(error) => Err(error),
        };
    }
    set_model(
        &connection,
        acp::SetSessionModelRequest::new(id, session_model).meta(model_meta(effort)),
    )
    .await
}

async fn set_effort_option(
    connection: &acp::ClientSideConnection,
    id: &acp::SessionId,
    effort: &str,
) -> Result<(), EffortSetupError> {
    let request = acp::SetSessionConfigOptionRequest::new(
        id.clone(),
        "effort",
        acp::SessionConfigValueId::new(effort),
    );
    match tokio::time::timeout(
        EFFORT_SETUP_TIMEOUT,
        connection.set_session_config_option(request),
    )
    .await
    {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(EffortSetupError::Failed(error)),
        Err(_) => Err(EffortSetupError::TimedOut),
    }
}

async fn set_model(
    connection: &acp::ClientSideConnection,
    request: acp::SetSessionModelRequest,
) -> Result<(), EffortSetupError> {
    match tokio::time::timeout(EFFORT_SETUP_TIMEOUT, connection.set_session_model(request)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(EffortSetupError::Failed(error)),
        Err(_) => Err(EffortSetupError::TimedOut),
    }
}

fn model_meta(effort: Option<&str>) -> Option<Map<String, Value>> {
    effort.map(|effort| {
        Map::from_iter([(
            "reasoningEffort".to_owned(),
            Value::String(effort.to_owned()),
        )])
    })
}
