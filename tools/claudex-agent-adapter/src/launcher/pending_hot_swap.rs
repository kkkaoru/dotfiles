use std::fs;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{ServiceConfig, macos_notify};

mod state;
use state::{cache_dir, read_state, state_path, write_state};

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
        stop_waiter(existing.pid, waiter_is_alive);
        let _ = fs::remove_file(path);
    }
}

pub(super) fn disarm(config: &ServiceConfig) {
    let Ok(path) = state_path(config) else {
        return;
    };
    if let Ok(Some(existing)) = read_state(&path) {
        stop_waiter(existing.pid, waiter_is_alive);
    }
    let _ = fs::remove_file(path);
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

#[path = "pending_hot_swap_process.rs"]
mod process;
use process::{spawn_waiter, stop_waiter, waiter_is_alive};
#[cfg(test)]
use process::is_wait_idle_command_line;



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
