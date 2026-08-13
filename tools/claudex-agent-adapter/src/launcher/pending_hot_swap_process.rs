use std::{
    fs::{self, OpenOptions},
    io::Write,
    process::{Command, Stdio},
};

use anyhow::{Context, Result};

use super::{ServiceConfig, cache_dir};
use crate::launcher::{daemon_arguments, launcher_logs};

/// Owns a detached idle waiter until its PID is durably published in pending
/// state. Drop covers state-write failures and cancellation after spawning.
#[must_use = "a spawned waiter must stay guarded until pending state is published"]
pub(super) struct StartedWaiter<T: FnOnce(u32) = fn(u32)> {
    pid: u32,
    terminate: Option<T>,
}

impl StartedWaiter<fn(u32)> {
    fn new(pid: u32) -> Self {
        Self::with_terminate(pid, terminate_started_waiter)
    }
}

impl<T: FnOnce(u32)> StartedWaiter<T> {
    pub(super) fn with_terminate(pid: u32, terminate: T) -> Self {
        Self {
            pid,
            terminate: Some(terminate),
        }
    }

    pub(super) fn pid(&self) -> u32 {
        self.pid
    }

    pub(super) fn disarm(mut self) -> u32 {
        self.terminate.take();
        self.pid
    }
}

impl<T: FnOnce(u32)> Drop for StartedWaiter<T> {
    fn drop(&mut self) {
        if let Some(terminate) = self.terminate.take() {
            terminate(self.pid);
        }
    }
}

#[cfg(unix)]
pub(super) fn detached_waiter_group(pid: u32) -> Option<i32> {
    if pid == 0 || pid == std::process::id() || pid > i32::MAX as u32 {
        return None;
    }
    let pid = pid as i32;
    (unsafe { libc::getsid(pid) } == pid).then_some(pid)
}

#[cfg(unix)]
fn terminate_started_waiter(pid: u32) {
    let Some(process_group) = detached_waiter_group(pid) else {
        return;
    };
    unsafe {
        libc::kill(-process_group, libc::SIGTERM);
        libc::kill(-process_group, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn terminate_started_waiter(pid: u32) {
    stop_waiter(pid, |_| true);
}

pub(super) fn spawn_waiter(config: &ServiceConfig) -> Result<StartedWaiter> {
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
    Ok(StartedWaiter::new(child.id()))
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

#[cfg(any(test, not(unix)))]
pub(super) fn stop_waiter(pid: u32, is_alive: impl Fn(u32) -> bool) {
    if pid == 0 || pid == std::process::id() || !is_alive(pid) {
        return;
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

/// Force-clean a stale waiter and every process in its detached group.
pub(super) fn terminate_waiter_group(pid: u32, is_alive: impl Fn(u32) -> bool) {
    if !is_alive(pid) {
        return;
    }
    terminate_started_waiter(pid);
}

/// Ask the current waiter lifecycle to stop. SIGTERM targets the detached
/// group so a shell/wrapper cannot leave its wait-idle child behind.
pub(super) fn request_waiter_stop(pid: u32, is_alive: impl Fn(u32) -> bool) {
    if !is_alive(pid) {
        return;
    }
    #[cfg(unix)]
    if let Some(process_group) = detached_waiter_group(pid) {
        unsafe {
            libc::kill(-process_group, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    stop_waiter(pid, |_| true);
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
