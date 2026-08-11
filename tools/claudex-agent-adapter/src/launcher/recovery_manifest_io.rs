use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};

use super::{
    MANIFEST_PREFIX, MANIFEST_SUFFIX, RecoveryManifest, ServiceConfig,
    ensure_private_directory, set_private_permissions, validate_private_file,
};

pub(super) fn validate_arguments(manifest: &RecoveryManifest) -> Result<()> {
    ensure!(
        manifest.arguments.first().map(String::as_str) == Some("serve"),
        "recovery manifest is not a daemon command"
    );
    let listen = manifest
        .arguments
        .windows(2)
        .find(|pair| pair[0] == "--listen")
        .map(|pair| pair[1].as_str());
    let expected_listen = manifest.listen.to_string();
    ensure!(
        listen == Some(expected_listen.as_str()),
        "recovery manifest listener argument mismatch"
    );
    Ok(())
}

pub(super) fn publish_manifest(path: &Path, manifest: &RecoveryManifest) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .context("create adapter recovery manifest")?;
    set_private_permissions(&temporary, 0o600)?;
    output
        .write_all(&serde_json::to_vec(manifest).context("encode adapter recovery manifest")?)
        .context("write adapter recovery manifest")?;
    output
        .sync_all()
        .context("sync adapter recovery manifest")?;
    fs::rename(&temporary, path).context("publish adapter recovery manifest")?;
    validate_private_file(path, 0o600, "recovery manifest")
}

pub(super) fn manifest_file_name(generation: &str) -> String {
    format!("{MANIFEST_PREFIX}{generation}{MANIFEST_SUFFIX}")
}

pub(super) fn read_manifest(path: &Path) -> Result<RecoveryManifest> {
    serde_json::from_slice(&fs::read(path).context("read adapter recovery manifest")?)
        .context("decode adapter recovery manifest")
}

pub(super) fn recovery_root(config: &ServiceConfig) -> Result<PathBuf> {
    Ok(config
        .log_path
        .parent()
        .context("adapter log has no parent")?
        .join("recovery"))
}

pub(super) fn ensure_recovery_root(config: &ServiceConfig) -> Result<PathBuf> {
    let root = recovery_root(config)?;
    ensure_private_directory(&root)?;
    Ok(root)
}
