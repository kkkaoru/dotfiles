use std::{
    fs::{self, OpenOptions},
    process::{Command, Stdio},
};

use anyhow::{Context, Result};

use super::{ServiceConfig, daemon_arguments::daemon_arguments, launcher_logs};

pub(super) fn start_adapter(config: &ServiceConfig) -> Result<u32> {
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
    let child = crate::path_env::apply_daemon_env(
        command
            .arg(&config.executable)
            .args(daemon_arguments(&config.options)),
        &config.token,
    )
    .env(
        crate::app_server::CODEX_CONFIG_FINGERPRINT_ENV,
        &config.codex_config_fingerprint,
    )
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
