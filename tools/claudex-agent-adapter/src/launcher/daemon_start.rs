use std::path::Path;

use anyhow::Result;

use super::{
    RECOVERY_MANIFEST_ENV, SERVICE_CONFIG_FINGERPRINT_ENV, ServiceConfig,
    daemon_arguments::daemon_arguments, launcher_logs, recovery_manifest,
};

#[derive(Clone, Debug)]
pub(super) struct RecoveryProcess {
    pub(super) pid: u32,
    pub(super) generation: String,
    pub(super) protocol_version: u64,
    pub(super) build_id: String,
    pub(super) model: String,
    pub(super) codex_config_fingerprint: String,
    pub(super) service_config_fingerprint: String,
}

/// Owns a detached daemon until its PID has been durably handed to another
/// lifecycle owner. Keeping this guard armed across `await` points makes early
/// returns and task cancellation clean up a successful spawn automatically.
#[must_use = "a newly spawned daemon must stay guarded until ownership is published"]
pub(super) struct StartedDaemon<T: FnOnce(u32) = fn(u32)> {
    pid: u32,
    terminate: Option<T>,
}

impl StartedDaemon<fn(u32)> {
    pub(super) fn new(pid: u32) -> StartedDaemon<impl FnOnce(u32)> {
        StartedDaemon::with_terminate(pid, super::daemon_process::started_daemon_cleanup(pid))
    }
}

impl<T: FnOnce(u32)> StartedDaemon<T> {
    pub(super) fn with_terminate(pid: u32, terminate: T) -> Self {
        Self {
            pid,
            terminate: Some(terminate),
        }
    }

    pub(super) fn pid(&self) -> u32 {
        self.pid
    }

    /// Transfer ownership of the process to published launcher state.
    pub(super) fn disarm(mut self) -> u32 {
        self.terminate.take();
        self.pid
    }
}

impl<T: FnOnce(u32)> Drop for StartedDaemon<T> {
    fn drop(&mut self) {
        if let Some(terminate) = self.terminate.take() {
            terminate(self.pid);
        }
    }
}

pub(super) fn start_adapter(config: &ServiceConfig) -> Result<u32> {
    start_with_retained(config, None, config)
}

/// Start a listener on a temporary address while retaining the configured
/// service address as its lifecycle target. This makes the runtime treat the
/// listener as a fallback generation and retire it after the sticky idle grace.
pub(super) fn start_adapter_for_service(
    listen_config: &ServiceConfig,
    service_config: &ServiceConfig,
) -> Result<u32> {
    start_with_retained(listen_config, None, service_config)
}

pub(super) fn start_adapter_with_retained(
    listen_config: &ServiceConfig,
    retained_path: &Path,
    manifest_config: &ServiceConfig,
) -> Result<u32> {
    start_with_retained(listen_config, Some(retained_path), manifest_config)
}

fn start_with_retained(
    listen_config: &ServiceConfig,
    retained_path: Option<&Path>,
    manifest_config: &ServiceConfig,
) -> Result<u32> {
    let manifest_path = recovery_manifest::prepare(manifest_config)?;
    spawn_adapter(SpawnRequest {
        config: listen_config,
        executable: &listen_config.executable,
        arguments: daemon_arguments(&listen_config.options),
        codex_config_fingerprint: &listen_config.codex_config_fingerprint,
        service_config_fingerprint: &listen_config.service_config_fingerprint,
        manifest_path: Some(&manifest_path),
        retained_path,
        service_listen: manifest_config.options.listen,
    })
}

pub(super) fn start_ephemeral_adapter(config: &ServiceConfig) -> Result<u32> {
    spawn_adapter(SpawnRequest {
        config,
        executable: &config.executable,
        arguments: daemon_arguments(&config.options),
        codex_config_fingerprint: &config.codex_config_fingerprint,
        service_config_fingerprint: &config.service_config_fingerprint,
        manifest_path: None,
        retained_path: None,
        service_listen: config.options.listen,
    })
}

pub(super) fn validate_recovery(
    config: &ServiceConfig,
    generation: &str,
) -> Result<recovery_manifest::ValidatedRecovery> {
    recovery_manifest::validate(config, generation)
}

pub(super) fn start_recovery(config: &ServiceConfig, generation: &str) -> Result<RecoveryProcess> {
    let recovery = validate_recovery(config, generation)?;
    let pid = spawn_adapter(SpawnRequest {
        config,
        executable: &recovery.executable,
        arguments: recovery.arguments,
        codex_config_fingerprint: &recovery.codex_config_fingerprint,
        service_config_fingerprint: &recovery.service_config_fingerprint,
        manifest_path: Some(&recovery.manifest_path),
        retained_path: None,
        service_listen: config.options.listen,
    })?;
    Ok(RecoveryProcess {
        pid,
        generation: recovery.generation,
        protocol_version: recovery.protocol_version,
        build_id: recovery.build_id,
        model: recovery.model,
        codex_config_fingerprint: recovery.codex_config_fingerprint,
        service_config_fingerprint: recovery.service_config_fingerprint,
    })
}

#[cfg(test)]
pub(super) fn terminate_started_recovery(pid: u32) {
    super::daemon_process::terminate_started_daemon(pid);
}

#[path = "daemon_start_spawn.rs"]
mod spawn;
#[cfg(test)]
use spawn::configure_process_group;
use spawn::{SpawnRequest, spawn_adapter};

#[cfg(unix)]
#[path = "daemon_start_descriptors.rs"]
mod descriptors;
#[cfg(unix)]
use descriptors::detach_session_and_close_inherited_descriptors;
#[cfg(all(test, unix))]
use descriptors::{
    bounded_descriptor_limit, close_file_descriptor, close_inherited_descriptors_with, close_system,
};

#[cfg(test)]
#[path = "daemon_start_tests.rs"]
mod tests;
