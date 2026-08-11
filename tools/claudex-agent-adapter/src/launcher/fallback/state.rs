use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use serde_json;

use super::super::ServiceConfig;
use super::{FallbackState, STATE_PREFIX, STATE_SUFFIX};

pub(super) fn state_path(config: &ServiceConfig) -> Result<PathBuf> {
    let parent = config
        .log_path
        .parent()
        .context("adapter log has no parent")?;
    Ok(parent.join(format!(
        "{STATE_PREFIX}{}{}",
        config.options.listen.port(),
        STATE_SUFFIX
    )))
}

pub(super) fn read_state(path: &PathBuf) -> Result<Option<FallbackState>> {
    if !path.exists() {
        return Ok(None);
    }
    let state: FallbackState =
        serde_json::from_slice(&fs::read(path).context("read current-build fallback state")?)
            .context("decode current-build fallback state")?;
    if !state.listen.ip().is_loopback() || state.listen.port() == 0 || state.pid == 0 {
        bail!("invalid current-build fallback state");
    }
    Ok(Some(state))
}

pub(super) fn write_state(path: &PathBuf, state: &FallbackState) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .context("create current-build fallback state")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
            .context("secure current-build fallback state")?;
    }
    output
        .write_all(&serde_json::to_vec(state).context("encode current-build fallback state")?)
        .context("write current-build fallback state")?;
    output
        .sync_all()
        .context("sync current-build fallback state")?;
    fs::rename(&temporary, path).context("publish current-build fallback state")
}
