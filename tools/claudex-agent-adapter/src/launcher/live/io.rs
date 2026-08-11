use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::{LiveState, RetainedGeneration};
use super::super::{ServiceConfig, launcher_logs};

pub(super) fn state_path(config: &ServiceConfig) -> Result<PathBuf> {
    Ok(launcher_logs::live_state_path(
        cache_dir(config)?,
        &config.options.listen,
    ))
}

pub(super) fn retained_path(config: &ServiceConfig) -> Result<PathBuf> {
    Ok(launcher_logs::retained_state_path(
        cache_dir(config)?,
        &config.options.listen,
    ))
}

pub(super) fn cache_dir(config: &ServiceConfig) -> Result<&Path> {
    config
        .log_path
        .parent()
        .context("adapter log has no parent")
}

pub(super) fn read_live(path: &Path) -> Result<Option<LiveState>> {
    if !path.exists() {
        return Ok(None);
    }
    let state: LiveState =
        serde_json::from_slice(&fs::read(path).context("read live adapter state")?)
            .context("decode live adapter state")?;
    if state.listen.port() == 0 || !state.listen.ip().is_loopback() || state.build_id.is_empty() {
        bail!("invalid live adapter state");
    }
    Ok(Some(state))
}

pub(crate) fn read_retained(path: &Path) -> Result<Option<RetainedGeneration>> {
    if !path.exists() {
        return Ok(None);
    }
    let state: RetainedGeneration =
        serde_json::from_slice(&fs::read(path).context("read retained generation state")?)
            .context("decode retained generation state")?;
    if state.listen.port() == 0 || !state.listen.ip().is_loopback() || state.pid == 0 {
        bail!("invalid retained generation state");
    }
    Ok(Some(state))
}

/// Drop one sticky session from a retained snapshot.
///
/// Empty `session_ids` still keep listen/pid so `release_idle_retained` can find
/// a busy orphan after the last sticky owner forgets. Deleting the file here
/// leaked retained daemons that still had active work for other traffic.
pub(crate) fn forget_retained_session(path: &Path, session_id: &str) -> Result<()> {
    let Some(mut generation) = read_retained(path)? else {
        return Ok(());
    };
    let before = generation.session_ids.len();
    generation.session_ids.retain(|owned| owned != session_id);
    if generation.session_ids.len() == before {
        return Ok(());
    }
    write_json(path, &generation)
}

/// Remove the retained snapshot entirely (idle / dead generation recovery).
pub(crate) fn clear_retained(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).context("remove retained generation state")?;
    }
    Ok(())
}

pub(crate) fn load_retained(config: &ServiceConfig) -> Option<(PathBuf, RetainedGeneration)> {
    let path = retained_path(config).ok()?;
    let generation = read_retained(&path).ok().flatten()?;
    Some((path, generation))
}

pub(super) fn write_live(path: &Path, state: &LiveState) -> Result<()> {
    write_json(path, state)
}

pub(super) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .context("create live adapter state")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .context("secure live adapter state")?;
    }
    output
        .write_all(&serde_json::to_vec(value).context("encode live adapter state")?)
        .context("write live adapter state")?;
    output.sync_all().context("sync live adapter state")?;
    fs::rename(&temporary, path).context("publish live adapter state")
}
