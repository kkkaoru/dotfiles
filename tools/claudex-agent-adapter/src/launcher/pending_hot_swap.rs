use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{ServiceConfig, daemon_arguments, launcher_logs};

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
    if let Some(existing) = read_state(&path)? {
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
mod tests {
    use std::net::SocketAddr;
    use std::path::{Path, PathBuf};

    use super::super::{LOCAL_TOKEN, ServiceConfig};
    use super::*;
    use crate::agent_backend::{BackendKind, BackendRoute};
    use crate::launcher::AdapterOptions;

    fn config(root: &Path, listen: SocketAddr) -> ServiceConfig {
        ServiceConfig {
            options: AdapterOptions {
                routes: vec![BackendRoute::new("test-model", BackendKind::CodexAppServer)],
                listen,
                model: "test-model".to_owned(),
                subscription_max_processes: 20,
                subscription_timeout_minutes: 120,
                subagent_hard_timeout_seconds: None,
                model_catalog: crate::provider_config::ModelCatalog::default(),
            },
            token: LOCAL_TOKEN.to_owned(),
            codex_config_fingerprint: "test-fingerprint".to_owned(),
            service_config_fingerprint: "service-fingerprint".to_owned(),
            executable: PathBuf::from("/tmp/claudex-agent-adapter"),
            log_path: root.join("adapter.log"),
            lock_path: root.join("adapter.lock"),
        }
    }

    #[test]
    fn recognizes_wait_idle_command_lines_including_nohup() {
        assert!(is_wait_idle_command_line(
            "/Users/test/.cargo/bin/claudex-agent-adapter hot-swap --wait-idle --listen 127.0.0.1:8318"
        ));
        assert!(is_wait_idle_command_line(
            "nohup /Users/test/.cargo/bin/claudex-agent-adapter hot-swap --wait-idle --listen 127.0.0.1:8318"
        ));
        assert!(!is_wait_idle_command_line(
            "/Users/test/.cargo/bin/claudex-agent-adapter serve --listen 127.0.0.1:8318"
        ));
        assert!(!is_wait_idle_command_line(
            "/Users/test/.cargo/bin/claudex-agent-adapter hot-swap --listen 127.0.0.1:8318"
        ));
        assert!(!is_wait_idle_command_line(
            "/Users/test/.cargo/bin/claudex-agent-adapter launch --wait-idle --"
        ));
    }

    #[test]
    fn arms_once_per_live_build_and_respawns_stale_waiters() {
        let root = tempfile::tempdir().expect("pending hot-swap fixture");
        let config = config(root.path(), "127.0.0.1:8318".parse().expect("listen"));
        let first = arm_with(&config, |_| Ok(4242), |_| false).expect("first arm");
        assert_eq!(first, ArmOutcome::Spawned { pid: 4242 });
        let again = arm_with(&config, |_| Ok(4343), |pid| pid == 4242).expect("reuse live waiter");
        assert_eq!(again, ArmOutcome::AlreadyArmed { pid: 4242 });
        let respawn = arm_with(&config, |_| Ok(4444), |_| false).expect("respawn dead waiter");
        assert_eq!(respawn, ArmOutcome::Spawned { pid: 4444 });
        let state = read_state_for_tests(&config)
            .expect("read pending")
            .expect("pending after respawn");
        assert_eq!(state.pid, 4444);
        assert_eq!(state.build_id, env!("CLAUDEX_BUILD_ID"));
        clear_if_current(&config);
        assert!(read_state_for_tests(&config).expect("cleared").is_none());
    }

    #[test]
    fn pending_state_round_trips_build_and_pid() {
        let state = PendingHotSwap {
            build_id: "build".to_owned(),
            service_config_fingerprint: "service".to_owned(),
            pid: 42,
        };
        let encoded = serde_json::to_string(&state).expect("encode");
        let decoded: PendingHotSwap = serde_json::from_str(&encoded).expect("decode");
        assert_eq!(decoded, state);
    }

    #[test]
    fn arm_outcome_pid_matches_each_variant() {
        assert_eq!(ArmOutcome::AlreadyArmed { pid: 7 }.pid(), 7);
        assert_eq!(ArmOutcome::Spawned { pid: 9 }.pid(), 9);
    }

    #[test]
    fn respawns_when_fingerprint_changes_and_ignores_foreign_builds() {
        let root = tempfile::tempdir().expect("pending hot-swap fixture");
        let config = config(root.path(), "127.0.0.1:8319".parse().expect("listen"));
        arm_with(&config, |_| Ok(5151), |_| false).expect("initial arm");
        let path = state_path(&config).expect("state path");
        write_state(
            &path,
            &PendingHotSwap {
                build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
                service_config_fingerprint: "other-fingerprint".to_owned(),
                pid: 5151,
            },
        )
        .expect("stale fingerprint");
        let respawn =
            arm_with(&config, |_| Ok(5252), |pid| pid == 5151).expect("fingerprint change");
        assert_eq!(respawn, ArmOutcome::Spawned { pid: 5252 });

        write_state(
            &path,
            &PendingHotSwap {
                build_id: "other-build".to_owned(),
                service_config_fingerprint: config.service_config_fingerprint.clone(),
                pid: 5252,
            },
        )
        .expect("foreign build");
        clear_if_current(&config);
        assert_eq!(
            read_state_for_tests(&config)
                .expect("foreign build remains")
                .expect("pending")
                .build_id,
            "other-build"
        );
    }

    #[test]
    fn stop_waiter_ignores_missing_self_and_dead_pids() {
        stop_waiter(0, |_| true);
        stop_waiter(std::process::id(), |_| true);
        stop_waiter(1, |_| false);
    }
}
