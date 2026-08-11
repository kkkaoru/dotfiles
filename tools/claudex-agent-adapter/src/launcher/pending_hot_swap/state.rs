use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use super::PendingHotSwap;
use super::super::{ServiceConfig, launcher_logs};

pub(super) fn cache_dir(config: &ServiceConfig) -> Result<&Path> {
    config
        .log_path
        .parent()
        .context("adapter log has no parent")
}

pub(super) fn state_path(config: &ServiceConfig) -> Result<PathBuf> {
    Ok(launcher_logs::pending_hot_swap_state_path(
        cache_dir(config)?,
        &config.options.listen,
    ))
}

pub(super) fn read_state(path: &Path) -> Result<Option<PendingHotSwap>> {
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

pub(super) fn write_state(path: &Path, state: &PendingHotSwap) -> Result<()> {
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
