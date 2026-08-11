use anyhow::{Context, Result};

use super::super::{
    ServiceConfig, daemon_start, handover::ServiceState, live, macos_notify,
};

pub(in crate::launcher) fn notify_live_listener(config: &ServiceConfig, url: &str) {
    if let Ok(live_listen) = live::parse_listen_url(url) {
        macos_notify::live_ready(config, live_listen);
    }
}

pub(in crate::launcher) fn log_live_listener(config: &ServiceConfig) {
    if let Ok(Some(status)) = live::read(config) {
        eprintln!(
            "claudex: live generation {} on {}",
            status.build_id, status.listen
        );
    }
}

pub(in crate::launcher) fn should_retry_idle_replace(failures: u32, limit: Option<u32>) -> bool {
    limit.is_none_or(|limit| failures <= limit)
}

pub(in crate::launcher) fn listener_was_replaced(state: &ServiceState) -> bool {
    matches!(state, ServiceState::Replace { .. })
}

pub(in crate::launcher) fn notify_swap_if_replaced(replaced: bool, config: &ServiceConfig) {
    if replaced {
        macos_notify::swap_complete(config);
    }
}

pub(in crate::launcher) fn usable_recovery_generation(
    config: &ServiceConfig,
    generation: Option<&str>,
) -> Result<Option<String>> {
    let Some(generation) = generation else {
        eprintln!(
            "claudex: current adapter predates recovery generations; performing a one-time preflight-only migration"
        );
        return Ok(None);
    };
    match daemon_start::validate_recovery(config, generation) {
        Ok(_) => Ok(Some(generation.to_owned())),
        Err(error) if recovery_snapshot_is_missing(&error) => {
            eprintln!(
                "claudex: recovery snapshot `{generation}` is unavailable ({error:#}); performing a preflight-only migration"
            );
            Ok(None)
        }
        Err(error) => {
            Err(error).context("validate current adapter recovery generation before handover")
        }
    }
}

pub(super) fn recovery_snapshot_is_missing(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == std::io::ErrorKind::NotFound)
    })
}
