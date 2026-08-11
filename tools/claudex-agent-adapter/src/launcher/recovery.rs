use std::future::Future;

use anyhow::{Error, Result, anyhow};

use super::{
    ServiceConfig,
    daemon_start::{self, RecoveryProcess},
    wait_until_recovery_ready,
};

pub(super) async fn after_update_failure(
    client: &reqwest::Client,
    config: &ServiceConfig,
    generation: Option<&str>,
    update_error: Error,
) -> Result<String> {
    let update_message = format!("{update_error:#}");
    recover_with(
        generation,
        update_error,
        |generation| daemon_start::start_recovery(config, generation),
        |recovery| async move { wait_until_recovery_ready(client, config, &recovery).await },
        daemon_start::terminate_started_recovery,
    )
    .await?;
    // Previous generation is serving again, but the requested update still
    // failed. Keep a non-zero ensure/update exit so callers do not treat the
    // new generation as current.
    Err(anyhow!(
        "{update_message}: restored previous generation; {} is still serving",
        config.base_url()
    ))
}

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
    if let Err(recovery_error) = wait(recovery).await {
        stop(recovery_pid);
        return Err(update_error.context(format!(
            "new adapter failed readiness and previous generation recovery failed: {recovery_error:#}"
        )));
    }
    eprintln!(
        "claudex: new adapter failed readiness; restored previous generation pid {recovery_pid}"
    );
    Ok(())
}

#[cfg(test)]
include!("recovery_tests.rs");
