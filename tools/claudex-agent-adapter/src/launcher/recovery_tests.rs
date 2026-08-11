#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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
        recover_with(
            Some("generation"),
            anyhow!("new generation failed"),
            start_expected_recovery,
            wait_for_expected_recovery,
            unexpected_stop,
        )
        .await
        .expect("successful recovery should keep the restored listener usable");
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
