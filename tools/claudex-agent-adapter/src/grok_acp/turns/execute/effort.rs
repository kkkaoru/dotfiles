use anyhow::anyhow;

use super::{EFFORT_SETUP_TIMEOUT, EffortSetupError, TurnCtl};
use crate::grok_acp::updates;

pub(super) fn finish_effort_setup(
    ctl: &mut TurnCtl<'_>,
    setup_result: Result<(), EffortSetupError>,
) -> bool {
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
