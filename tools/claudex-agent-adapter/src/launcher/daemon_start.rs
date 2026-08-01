use std::{
    fs::{self, OpenOptions},
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result};

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

pub(super) fn start_adapter(config: &ServiceConfig) -> Result<u32> {
    let manifest_path = recovery_manifest::prepare(config)?;
    spawn_adapter(
        config,
        &config.executable,
        daemon_arguments(&config.options),
        &config.codex_config_fingerprint,
        &config.service_config_fingerprint,
        Some(&manifest_path),
    )
}

pub(super) fn start_ephemeral_adapter(config: &ServiceConfig) -> Result<u32> {
    spawn_adapter(
        config,
        &config.executable,
        daemon_arguments(&config.options),
        &config.codex_config_fingerprint,
        &config.service_config_fingerprint,
        None,
    )
}

pub(super) fn validate_recovery(
    config: &ServiceConfig,
    generation: &str,
) -> Result<recovery_manifest::ValidatedRecovery> {
    recovery_manifest::validate(config, generation)
}

pub(super) fn start_recovery(config: &ServiceConfig, generation: &str) -> Result<RecoveryProcess> {
    let recovery = validate_recovery(config, generation)?;
    let pid = spawn_adapter(
        config,
        &recovery.executable,
        recovery.arguments,
        &recovery.codex_config_fingerprint,
        &recovery.service_config_fingerprint,
        Some(&recovery.manifest_path),
    )?;
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

pub(super) fn terminate_started_recovery(pid: u32) {
    super::daemon_process::terminate(pid);
}

fn spawn_adapter(
    config: &ServiceConfig,
    executable: &Path,
    arguments: Vec<std::ffi::OsString>,
    codex_config_fingerprint: &str,
    service_config_fingerprint: &str,
    manifest_path: Option<&Path>,
) -> Result<u32> {
    let log_dir = config
        .log_path
        .parent()
        .context("adapter log has no parent")?;
    fs::create_dir_all(log_dir).context("create adapter log directory")?;
    launcher_logs::archive_previous_log(&config.log_path)?;
    let mut stdout = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&config.log_path)
        .context("open adapter log")?;
    launcher_logs::write_adapter_log_header(
        &mut stdout,
        &config.options.model,
        &config.options.listen,
        config.token.len(),
    )?;
    let stderr = stdout.try_clone().context("clone adapter log handle")?;
    let mut command = Command::new("nohup");
    configure_process_group(&mut command);
    let command =
        crate::path_env::apply_daemon_env(command.arg(executable).args(arguments), &config.token)
            .env(
                crate::app_server::CODEX_CONFIG_FINGERPRINT_ENV,
                codex_config_fingerprint,
            )
            .env(SERVICE_CONFIG_FINGERPRINT_ENV, service_config_fingerprint);
    if let Some(manifest_path) = manifest_path {
        command.env(RECOVERY_MANIFEST_ENV, manifest_path);
    } else {
        command.env_remove(RECOVERY_MANIFEST_ENV);
    }
    let child = command
        .env_remove(crate::anthropic::SUBAGENT_HARD_TIMEOUT_ENV)
        .env_remove(crate::anthropic::LEGACY_SUBAGENT_RESPONSE_TIMEOUT_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .context("start adapter daemon")?;
    Ok(child.id())
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.process_group(0);
    // `ensure` may itself be running under `Command::output()`. Close
    // inherited non-stdio descriptors before `nohup` execs the daemon so
    // the caller's output pipes reach EOF when the launcher exits.
    unsafe {
        command.pre_exec(close_inherited_descriptors);
    }
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
type Io<T> = std::io::Result<T>;

#[cfg(unix)]
// Keep the pre-exec entrypoint as a single delegation; the range logic is tested below.
#[rustfmt::skip]
fn close_inherited_descriptors() -> Io<()> { close_system(close_file_descriptor) }

#[cfg(unix)]
fn close_system(close: impl FnMut(i32)) -> Io<()> {
    close_inherited_descriptors_with(unsafe { libc::sysconf(libc::_SC_OPEN_MAX) }, close)
}

#[cfg(unix)]
fn bounded_descriptor_limit(max_fd: libc::c_long) -> i32 {
    if max_fd > 3 && max_fd < 1_048_576 {
        max_fd as i32
    } else {
        1024
    }
}

#[cfg(unix)]
fn close_inherited_descriptors_with(
    max_fd: libc::c_long,
    close: impl FnMut(i32),
) -> std::io::Result<()> {
    close_descriptors_up_to(bounded_descriptor_limit(max_fd), close);
    Ok(())
}

#[cfg(unix)]
fn close_descriptors_up_to(max_fd: i32, mut close: impl FnMut(i32)) {
    for fd in 3..max_fd {
        close(fd);
    }
}

#[cfg(unix)]
fn close_file_descriptor(fd: i32) {
    unsafe {
        libc::close(fd);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{
        bounded_descriptor_limit, close_file_descriptor, close_inherited_descriptors_with,
        close_system,
    };

    #[test]
    fn bounds_descriptor_limits_before_closing_inherited_fds() {
        assert_eq!(bounded_descriptor_limit(4), 4);
        assert_eq!(bounded_descriptor_limit(3), 1024);
        assert_eq!(bounded_descriptor_limit(1_048_576), 1024);
    }

    #[test]
    fn closes_the_inherited_descriptor_range_via_the_injected_operation() {
        let mut closed = Vec::new();
        close_inherited_descriptors_with(6, |fd| closed.push(fd))
            .expect("close operation is infallible");
        assert_eq!(closed, vec![3, 4, 5]);
        close_system(|_| {}).expect("system descriptor limit is available");
        close_file_descriptor(-1);
    }
}
