use std::fs;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{ServiceConfig, macos_notify};

mod state;
#[cfg(test)]
use state::FailStateWrite;
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
        return arm_with(
            config,
            |_| Ok(StartedWaiter::with_terminate(pid, |_| {})),
            |_| false,
        );
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
        request_waiter_stop(existing.pid, waiter_is_alive);
        let _ = fs::remove_file(path);
    }
}

pub(super) fn disarm(config: &ServiceConfig) {
    let Ok(path) = state_path(config) else {
        return;
    };
    if let Ok(Some(existing)) = read_state(&path) {
        request_waiter_stop(existing.pid, waiter_is_alive);
    }
    let _ = fs::remove_file(path);
}

fn arm_with<T: FnOnce(u32)>(
    config: &ServiceConfig,
    spawn: impl FnOnce(&ServiceConfig) -> Result<StartedWaiter<T>>,
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
        terminate_waiter_group(existing.pid, &is_alive);
        let _ = fs::remove_file(&path);
    }
    let started = spawn(config)?;
    write_state(
        &path,
        &PendingHotSwap {
            build_id: env!("CLAUDEX_BUILD_ID").to_owned(),
            service_config_fingerprint: config.service_config_fingerprint.clone(),
            pid: started.pid(),
        },
    )?;
    let pid = started.disarm();
    macos_notify::waiting_for_idle(config, pid);
    Ok(ArmOutcome::Spawned { pid })
}

#[path = "pending_hot_swap_process.rs"]
mod process;
#[cfg(test)]
use process::is_wait_idle_command_line;
#[cfg(test)]
use process::stop_waiter;
use process::{
    StartedWaiter, request_waiter_stop, spawn_waiter, terminate_waiter_group, waiter_is_alive,
};

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
#[path = "pending_hot_swap_process_live_tests.rs"]
mod process_live_tests;
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "pending_hot_swap_tests.rs"]
mod tests;
