use std::{cell::Cell, rc::Rc};

use agent_client_protocol as acp;

use super::{TurnCtl, finish_effort_setup, handle_setup_cancellation};

#[path = "setup_effort.rs"]
mod effort;
use effort::{
    effort_already_applied, forget_applied_effort, remember_applied_effort, setup_effort,
};

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
    if ctl.provider.model_is_launch_scoped() {
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
    let Some(effort) = effort else {
        tracing::info!(
            session_id = ctl.session_id,
            provider = ctl.provider.label(),
            "skipping ACP set_session_model; session/new already pinned the model"
        );
        return true;
    };
    if effort_already_applied(ctl.session_id, effort) {
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
        Some(effort),
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
        remember_applied_effort(ctl.session_id, effort);
        tokio::task::yield_now().await;
    }
    finish_effort_setup(ctl, setup_result)
}
