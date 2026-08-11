use std::{
    fs::{self, OpenOptions},
    io::Write,
    process::{Command, Stdio},
};

use anyhow::{Context, Result};

use super::{ServiceConfig, cache_dir};
use crate::launcher::{daemon_arguments, launcher_logs};

pub(super) fn spawn_waiter(config: &ServiceConfig) -> Result<u32> {
    let cache = cache_dir(config)?;
    fs::create_dir_all(cache).context("create pending hot-swap log directory")?;
    let log_path = launcher_logs::pending_hot_swap_log_path(cache, &config.options.listen);
    let mut stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .context("open pending hot-swap waiter log")?;
    writeln!(
        stdout,
        "=== pending hot-swap waiter start build={} listen={} ===",
        env!("CLAUDEX_BUILD_ID"),
        config.options.listen
    )
    .context("write pending hot-swap waiter log header")?;
    let stderr = stdout
        .try_clone()
        .context("clone pending hot-swap waiter log")?;
    let mut command = Command::new("nohup");
    configure_detached_session(&mut command);
    let child = crate::path_env::apply_daemon_env(
        command
            .arg(&config.executable)
            .args(daemon_arguments::hot_swap_wait_arguments(&config.options)),
        &config.token,
    )
    .env_remove(super::super::macos_notify_dispatch::MACOS_NOTIFY_ENV)
    .stdin(Stdio::null())
    .stdout(Stdio::from(stdout))
    .stderr(Stdio::from(stderr))
    .spawn()
    .context("start idle hot-swap waiter")?;
    Ok(child.id())
}

#[cfg(unix)]
pub(super) fn configure_detached_session(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_detached_session(_command: &mut Command) {}

pub(super) fn stop_waiter(pid: u32, is_alive: impl Fn(u32) -> bool) {
    if pid == 0 || pid == std::process::id() || !is_alive(pid) {
        return;
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

pub(super) fn waiter_is_alive(pid: u32) -> bool {
    process_command_line(pid).is_some_and(|command| is_wait_idle_command_line(&command))
}

pub(super) fn process_command_line(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|command| !command.is_empty())
}

pub(super) fn is_wait_idle_command_line(command: &str) -> bool {
    let mut fields = command.split_whitespace();
    let Some(first) = fields.next() else {
        return false;
    };
    let executable = if first.rsplit('/').next() == Some("nohup") {
        match fields.next() {
            Some(next) => next,
            None => return false,
        }
    } else {
        first
    };
    if executable.rsplit('/').next() != Some("claudex-agent-adapter") {
        return false;
    }
    let mut saw_hot_swap = false;
    let mut saw_wait_idle = false;
    for field in fields {
        saw_hot_swap |= field == "hot-swap";
        saw_wait_idle |= field == "--wait-idle";
    }
    saw_hot_swap && saw_wait_idle
}
