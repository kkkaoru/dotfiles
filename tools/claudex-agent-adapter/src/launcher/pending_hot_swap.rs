use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{ServiceConfig, daemon_arguments, launcher_logs, macos_notify};

#[cfg(test)]
use std::cell::Cell;

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub(super) struct PendingHotSwap {
    pub(super) build_id: String,
    pub(super) service_config_fingerprint: String,
    pub(super) pid: u32,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ArmOutcome {
    AlreadyArmed { pid: u32 },
    Spawned { pid: u32 },
}

impl ArmOutcome {
    pub(super) fn pid(&self) -> u32 {
        match self {
            Self::AlreadyArmed { pid } | Self::Spawned { pid } => *pid,
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_SPAWN_PID: Cell<Option<u32>> = const { Cell::new(None) };
}

pub(super) fn arm(config: &ServiceConfig) -> Result<ArmOutcome> {
    #[cfg(test)]
    if let Some(pid) = TEST_SPAWN_PID.with(Cell::get) {
        return arm_with(config, |_| Ok(pid), |_| false);
    }
    arm_with(config, spawn_waiter, waiter_is_alive)
}

pub(super) fn clear_if_current(config: &ServiceConfig) {
    let Ok(path) = state_path(config) else {
        return;
    };
    if let Ok(Some(existing)) = read_state(&path)
        && existing.build_id == env!("CLAUDEX_BUILD_ID")
    {
        let _ = fs::remove_file(path);
    }
}

fn arm_with(
    config: &ServiceConfig,
    spawn: impl FnOnce(&ServiceConfig) -> Result<u32>,
    is_alive: impl Fn(u32) -> bool,
) -> Result<ArmOutcome> {
    let path = state_path(config)?;
    let existing = match read_state(&path) {
        Ok(existing) => existing,
        Err(_) => {
            let _ = fs::remove_file(&path);
            None
        }
    };
    if let Some(existing) = existing {
        if existing.build_id == env!("CLAUDEX_BUILD_ID")
            && existing.service_config_fingerprint == config.service_config_fingerprint
            && is_alive(existing.pid)
        {
            return Ok(ArmOutcome::AlreadyArmed { pid: existing.pid });
        }
        stop_waiter(existing.pid, &is_alive);
        let _ = fs::remove_file(&path);
    }
    let pid = spawn(config)?;
    write_state(
        &path,
        &PendingHotSwap {
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            service_config_fingerprint: config.service_config_fingerprint.clone(),
            pid,
        },
    )?;
    macos_notify::waiting_for_idle(config, pid);
    Ok(ArmOutcome::Spawned { pid })
}

fn spawn_waiter(config: &ServiceConfig) -> Result<u32> {
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
    .stdin(Stdio::null())
    .stdout(Stdio::from(stdout))
    .stderr(Stdio::from(stderr))
    .spawn()
    .context("start idle hot-swap waiter")?;
    Ok(child.id())
}

#[cfg(unix)]
fn configure_detached_session(command: &mut Command) {
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

fn stop_waiter(pid: u32, is_alive: impl Fn(u32) -> bool) {
    if pid == 0 || pid == std::process::id() || !is_alive(pid) {
        return;
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

fn waiter_is_alive(pid: u32) -> bool {
    process_command_line(pid).is_some_and(|command| is_wait_idle_command_line(&command))
}

fn process_command_line(pid: u32) -> Option<String> {
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

fn is_wait_idle_command_line(command: &str) -> bool {
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

fn cache_dir(config: &ServiceConfig) -> Result<&Path> {
    config
        .log_path
        .parent()
        .context("adapter log has no parent")
}

fn state_path(config: &ServiceConfig) -> Result<PathBuf> {
    Ok(launcher_logs::pending_hot_swap_state_path(
        cache_dir(config)?,
        &config.options.listen,
    ))
}

fn read_state(path: &Path) -> Result<Option<PendingHotSwap>> {
    if !path.exists() {
        return Ok(None);
    }
    let state: PendingHotSwap =
        serde_json::from_slice(&fs::read(path).context("read pending hot-swap state")?)
            .context("decode pending hot-swap state")?;
    if state.pid == 0 || state.build_id.is_empty() {
        anyhow::bail!("invalid pending hot-swap state");
    }
    Ok(Some(state))
}

fn write_state(path: &Path, state: &PendingHotSwap) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .context("create pending hot-swap state")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .context("secure pending hot-swap state")?;
    }
    output
        .write_all(&serde_json::to_vec(state).context("encode pending hot-swap state")?)
        .context("write pending hot-swap state")?;
    output.sync_all().context("sync pending hot-swap state")?;
    fs::rename(&temporary, path).context("publish pending hot-swap state")
}

#[cfg(test)]
pub(super) struct TestSpawnPid;

#[cfg(test)]
impl TestSpawnPid {
    pub(super) fn arm(pid: u32) -> Self {
        TEST_SPAWN_PID.with(|cell| cell.set(Some(pid)));
        Self
    }
}

#[cfg(test)]
impl Drop for TestSpawnPid {
    fn drop(&mut self) {
        TEST_SPAWN_PID.with(|cell| cell.set(None));
    }
}

#[cfg(test)]
pub(super) fn read_state_for_tests(config: &ServiceConfig) -> Result<Option<PendingHotSwap>> {
    read_state(&state_path(config)?)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "pending_hot_swap_tests.rs"]
mod tests;
