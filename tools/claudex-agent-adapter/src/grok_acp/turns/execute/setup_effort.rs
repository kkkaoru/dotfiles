use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use agent_client_protocol::{self as acp, Agent as _};
use serde_json::{Map, Value};

use super::super::{EFFORT_SETUP_TIMEOUT, EffortSetupError};
use crate::grok_acp::connection::AcpProvider;

thread_local! {
    static APPLIED_SESSION_EFFORT: RefCell<HashMap<String, String>> =
        RefCell::new(HashMap::new());
}

pub(super) fn effort_already_applied(session_id: &str, effort: &str) -> bool {
    APPLIED_SESSION_EFFORT
        .with(|applied| applied.borrow().get(session_id).map(String::as_str) == Some(effort))
}

pub(super) fn remember_applied_effort(session_id: &str, effort: &str) {
    APPLIED_SESSION_EFFORT.with(|applied| {
        applied
            .borrow_mut()
            .insert(session_id.to_owned(), effort.to_owned());
    });
}

pub(super) fn forget_applied_effort(session_id: &str) {
    APPLIED_SESSION_EFFORT.with(|applied| {
        applied.borrow_mut().remove(session_id);
    });
}

pub(super) fn effort_option_rejected(error: &acp::Error) -> bool {
    matches!(
        error.code,
        acp::ErrorCode::MethodNotFound | acp::ErrorCode::InvalidParams
    )
}

pub(super) async fn setup_effort(
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

pub(super) async fn set_effort_option(
    connection: &acp::ClientSideConnection,
    id: &acp::SessionId,
    effort: &str,
) -> Result<(), EffortSetupError> {
    let request = acp::SetSessionConfigOptionRequest::new(
        id.clone(),
        "effort",
        acp::SessionConfigValueId::new(effort),
    );
    map_effort_setup_result(
        tokio::time::timeout(
            EFFORT_SETUP_TIMEOUT,
            connection.set_session_config_option(request),
        )
        .await,
    )
    .map(|_| ())
}

fn map_effort_setup_result<T>(
    result: Result<Result<T, acp::Error>, tokio::time::error::Elapsed>,
) -> Result<T, EffortSetupError> {
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(EffortSetupError::Failed(error)),
        Err(_) => Err(EffortSetupError::TimedOut),
    }
}

pub(super) async fn set_model(
    connection: &acp::ClientSideConnection,
    request: acp::SetSessionModelRequest,
) -> Result<(), EffortSetupError> {
    map_effort_setup_result(
        tokio::time::timeout(EFFORT_SETUP_TIMEOUT, connection.set_session_model(request)).await,
    )
    .map(|_| ())
}

pub(super) fn model_meta(effort: Option<&str>) -> Option<Map<String, Value>> {
    effort.map(|effort| {
        Map::from_iter([(
            "reasoningEffort".to_owned(),
            Value::String(effort.to_owned()),
        )])
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn map_effort_setup_result_covers_ok_error_and_timeout() {
        assert!(map_effort_setup_result(Ok(Ok(()))).is_ok());
        assert!(matches!(
            map_effort_setup_result::<()>(Ok(Err(acp::Error::internal_error()))),
            Err(EffortSetupError::Failed(_))
        ));
        let timed_out = effort_setup_timeout_fixture().await;
        assert!(matches!(
            map_effort_setup_result(timed_out),
            Err(EffortSetupError::TimedOut)
        ));
    }

    async fn effort_setup_timeout_fixture()
    -> Result<Result<(), acp::Error>, tokio::time::error::Elapsed> {
        tokio::time::timeout(Duration::from_millis(1), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok::<(), acp::Error>(())
        })
        .await
    }
}
