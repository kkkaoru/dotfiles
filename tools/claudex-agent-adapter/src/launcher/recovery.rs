#[cfg(test)]
use std::future::Future;

use anyhow::{Error, Result, anyhow};

#[cfg(test)]
use super::daemon_start::RecoveryProcess;
use super::{
    ServiceConfig,
    daemon_start::{self, StartedDaemon},
    wait_until_recovery_ready,
};

pub(super) async fn after_update_failure(
    client: &reqwest::Client,
    config: &ServiceConfig,
    generation: Option<&str>,
    update_error: Error,
) -> Result<String> {
    let update_message = format!("{update_error:#}");
    let Some(generation) = generation else {
        return Err(update_error);
    };
    let recovery = daemon_start::start_recovery(config, generation).map_err(|recovery_error| {
        update_error.context(format!(
            "new adapter failed readiness and previous generation recovery failed: {recovery_error:#}"
        ))
    })?;
    let recovery_pid = recovery.pid;
    let started = StartedDaemon::new(recovery_pid);
    if let Err(recovery_error) = wait_until_recovery_ready(client, config, &recovery).await {
        return Err(anyhow!(update_message.clone()).context(format!(
            "new adapter failed readiness and previous generation recovery failed: {recovery_error:#}"
        )));
    }
    started.disarm();
    eprintln!(
        "claudex: new adapter failed readiness; restored previous generation pid {recovery_pid}"
    );
    // Previous generation is serving again, but the requested update still
    // failed. Keep a non-zero ensure/update exit so callers do not treat the
    // new generation as current.
    Err(anyhow!(
        "{update_message}: restored previous generation; {} is still serving",
        config.base_url()
    ))
}

#[cfg(test)]
async fn recover_with<Start, Wait, WaitFuture, Stop>(
    generation: Option<&str>,
    update_error: Error,
    start: Start,
    wait: Wait,
    stop: Stop,
) -> Result<()>
where
    Start: FnOnce(&str) -> Result<RecoveryProcess>,
    Wait: FnOnce(RecoveryProcess) -> WaitFuture,
    WaitFuture: Future<Output = Result<()>>,
    Stop: FnOnce(u32),
{
    let Some(generation) = generation else {
        return Err(update_error);
    };
    let recovery = match start(generation) {
        Ok(recovery) => recovery,
        Err(recovery_error) => {
            return Err(update_error.context(format!(
                "new adapter failed readiness and previous generation recovery failed: {recovery_error:#}"
            )));
        }
    };
    let recovery_pid = recovery.pid;
    let started = StartedDaemon::with_terminate(recovery_pid, stop);
    if let Err(recovery_error) = wait(recovery).await {
        return Err(update_error.context(format!(
            "new adapter failed readiness and previous generation recovery failed: {recovery_error:#}"
        )));
    }
    started.disarm();
    eprintln!(
        "claudex: new adapter failed readiness; restored previous generation pid {recovery_pid}"
    );
    Ok(())
}

#[cfg(test)]
include!("recovery_tests.rs");
