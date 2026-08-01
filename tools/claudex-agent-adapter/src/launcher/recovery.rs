use std::future::Future;

use anyhow::{Error, Result};

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
    recover_with(
        generation,
        update_error,
        |generation| daemon_start::start_recovery(config, generation),
        |recovery| async move { wait_until_recovery_ready(client, config, &recovery).await },
        daemon_start::terminate_started_recovery,
    )
    .await
}

async fn recover_with<Start, Wait, WaitFuture, Stop>(
    generation: Option<&str>,
    update_error: Error,
    start: Start,
    wait: Wait,
    stop: Stop,
) -> Result<String>
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
    Err(update_error.context(format!(
        "new adapter failed readiness; restored previous generation pid {recovery_pid}"
    )))
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::{RecoveryProcess, recover_with};

    fn recovery(pid: u32) -> RecoveryProcess {
        RecoveryProcess {
            pid,
            generation: "generation".to_owned(),
            protocol_version: 1,
            build_id: "build".to_owned(),
            model: "model".to_owned(),
            codex_config_fingerprint: "codex".to_owned(),
            service_config_fingerprint: "service".to_owned(),
        }
    }

    fn start_expected_recovery(generation: &str) -> anyhow::Result<RecoveryProcess> {
        assert_eq!(generation, "generation");
        Ok(recovery(77))
    }

    async fn wait_for_expected_recovery(process: RecoveryProcess) -> anyhow::Result<()> {
        assert_eq!(process.pid, 77);
        Ok(())
    }

    fn unexpected_start(_: &str) -> anyhow::Result<RecoveryProcess> {
        unreachable!()
    }

    async fn unexpected_wait(_: RecoveryProcess) -> anyhow::Result<()> {
        unreachable!()
    }

    fn unavailable_start(_: &str) -> anyhow::Result<RecoveryProcess> {
        Err(anyhow!("snapshot unavailable"))
    }

    async fn ready(_: RecoveryProcess) -> anyhow::Result<()> {
        Ok(())
    }

    async fn not_ready(_: RecoveryProcess) -> anyhow::Result<()> {
        Err(anyhow!("not ready"))
    }

    fn unexpected_stop(_: u32) {
        unreachable!()
    }

    #[tokio::test]
    async fn readiness_failure_restores_the_previous_generation() {
        let error = recover_with(
            Some("generation"),
            anyhow!("new generation failed"),
            start_expected_recovery,
            wait_for_expected_recovery,
            unexpected_stop,
        )
        .await
        .expect_err("successful recovery still reports the failed update");
        let message = format!("{error:#}");
        assert!(message.contains("restored previous generation pid 77"));
        assert!(message.contains("new generation failed"));
    }

    #[tokio::test]
    async fn readiness_failure_reports_absent_and_failed_recovery() {
        let absent = recover_with(
            None,
            anyhow!("new generation failed"),
            unexpected_start,
            unexpected_wait,
            unexpected_stop,
        )
        .await
        .expect_err("missing recovery remains failed");
        assert_eq!(absent.to_string(), "new generation failed");

        let failed = recover_with(
            Some("generation"),
            anyhow!("new generation failed"),
            unavailable_start,
            ready,
            unexpected_stop,
        )
        .await
        .expect_err("failed recovery is reported");
        let message = format!("{failed:#}");
        assert!(message.contains("recovery failed"));
        assert!(message.contains("snapshot unavailable"));

        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let observed = std::sync::Arc::clone(&stopped);
        let failed = recover_with(
            Some("generation"),
            anyhow!("new generation failed"),
            |_| Ok(recovery(88)),
            not_ready,
            move |pid| observed.store(pid, std::sync::atomic::Ordering::Relaxed),
        )
        .await
        .expect_err("failed recovered process must be stopped");
        assert!(format!("{failed:#}").contains("not ready"));
        assert_eq!(stopped.load(std::sync::atomic::Ordering::Relaxed), 88);
    }
}
