use std::{cell::Cell, rc::Rc};

use agent_client_protocol::{self as acp, Agent as _};
use serde_json::{Map, Value};

use super::{
    EFFORT_SETUP_TIMEOUT, EffortSetupError, TurnCtl, finish_effort_setup, handle_setup_cancellation,
};
use crate::grok_acp::connection::AcpProvider;
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
    if ctl.provider == AcpProvider::Grok {
        tracing::debug!(
            session_id = ctl.session_id,
            effort,
            "skipping ACP set_session_model; Grok model and effort are launch-scoped"
        );
        return true;
    }
    if ctl.provider.model_is_launch_scoped() && effort.is_none() {
        return true;
    }
    if effort.is_none() && !ctl.provider.is_session_scoped_configured() {
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
        tokio::task::yield_now().await;
    }
    finish_effort_setup(ctl, setup_result)
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
    // Launch-scoped providers already pass `--model` on the CLI and pin the ACP model id
    // during session/new. Only attempt an optional effort config option; do not reselect
    // the model (Cursor auto reselection is expensive and can re-route mid-session).
    if provider.model_is_launch_scoped() {
        let Some(effort) = effort else {
            return Ok(());
        };
        return match set_effort_option(&connection, &id, effort).await {
            Ok(()) => Ok(()),
            Err(EffortSetupError::Failed(error)) if effort_option_rejected(&error) => {
                tracing::debug!(
                    ?error,
                    "launch-scoped ACP rejected session effort option; continuing with CLI model"
                );
                Ok(())
            }
            Err(error) => Err(error),
        };
    }
    // Session-scoped configured ACP (OpenCode) and Copilot need the model selected before
    // effort options are available. Cursor's `auto` maps to ACP `default[]`.
    if provider.is_session_scoped_configured() {
        set_model(
            &connection,
            acp::SetSessionModelRequest::new(id.clone(), session_model.clone()),
        )
        .await?;
        let Some(effort) = effort else {
            return Ok(());
        };
        return match set_effort_option(&connection, &id, effort).await {
            Ok(()) => Ok(()),
            Err(EffortSetupError::Failed(error)) if effort_option_rejected(&error) => {
                tracing::debug!(
                    ?error,
                    "configured ACP rejected session effort option after model select; falling back to set_session_model meta"
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
