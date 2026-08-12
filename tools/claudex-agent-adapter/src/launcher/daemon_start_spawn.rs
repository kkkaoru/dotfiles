use std::fs::{self, OpenOptions};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

#[cfg(unix)]
use super::detach_session_and_close_inherited_descriptors;
use super::{RECOVERY_MANIFEST_ENV, SERVICE_CONFIG_FINGERPRINT_ENV, ServiceConfig, launcher_logs};

pub(super) struct SpawnRequest<'a> {
    pub(super) config: &'a ServiceConfig,
    pub(super) executable: &'a Path,
    pub(super) arguments: Vec<std::ffi::OsString>,
    pub(super) codex_config_fingerprint: &'a str,
    pub(super) service_config_fingerprint: &'a str,
    pub(super) manifest_path: Option<&'a Path>,
    pub(super) retained_path: Option<&'a Path>,
    pub(super) service_listen: std::net::SocketAddr,
}

pub(super) fn spawn_adapter(request: SpawnRequest<'_>) -> Result<u32> {
    let SpawnRequest {
        config,
        executable,
        arguments,
        codex_config_fingerprint,
        service_config_fingerprint,
        manifest_path,
        retained_path,
        service_listen,
    } = request;
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
    if let Some(retained_path) = retained_path {
        command.env(super::super::RETAINED_STATE_ENV, retained_path);
    } else {
        command.env_remove(super::super::RETAINED_STATE_ENV);
    }
    command.env(super::super::SERVICE_LISTEN_ENV, service_listen.to_string());
    let child = command
        .current_dir(log_dir)
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
pub(super) fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // `ensure` may itself be running under `Command::output()`. Close
    // inherited non-stdio descriptors before `nohup` execs the daemon so
    // the caller's output pipes reach EOF when the launcher exits.
    unsafe {
        // A process-group boundary alone still permits a parent/PTY teardown
        // to terminate the daemon. Creating a new session makes the daemon
        // independent of that lifecycle. `setsid` also makes the child its
        // own process-group leader, preserving targeted group shutdown via
        // `kill(-pid, ...)`.
        command.pre_exec(detach_session_and_close_inherited_descriptors);
    }
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}
