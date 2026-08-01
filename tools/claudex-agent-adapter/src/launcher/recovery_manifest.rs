use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{ServiceConfig, daemon_arguments::daemon_arguments};

const EXECUTABLE_NAME: &str = "claudex-agent-adapter";
const MANIFEST_PREFIX: &str = "manifest.";
const MANIFEST_SUFFIX: &str = ".json";
const RETAINED_GENERATIONS_PER_LISTENER: usize = 2;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct RecoveryManifest {
    generation: String,
    protocol_version: u64,
    build_id: String,
    listen: SocketAddr,
    model: String,
    arguments: Vec<String>,
    codex_config_fingerprint: String,
    service_config_fingerprint: String,
}

#[derive(Debug)]
pub(super) struct ValidatedRecovery {
    pub(super) generation: String,
    pub(super) manifest_path: PathBuf,
    pub(super) executable: PathBuf,
    pub(super) arguments: Vec<std::ffi::OsString>,
    pub(super) protocol_version: u64,
    pub(super) build_id: String,
    pub(super) model: String,
    pub(super) codex_config_fingerprint: String,
    pub(super) service_config_fingerprint: String,
}

pub(super) fn prepare(config: &ServiceConfig) -> Result<PathBuf> {
    let root = ensure_recovery_root(config)?;
    let build_id = env!("CLAUDEX_BUILD_ID");
    let build_directory = root.join(build_id);
    ensure_private_directory(&build_directory)?;
    let executable = build_directory.join(EXECUTABLE_NAME);
    if executable.exists() {
        validate_private_file(&executable, 0o700, "recovery executable")?;
    } else {
        let temporary = build_directory.join(format!(
            ".{EXECUTABLE_NAME}.{}.tmp",
            uuid::Uuid::new_v4().simple()
        ));
        fs::copy(&config.executable, &temporary).context("snapshot adapter executable")?;
        set_private_permissions(&temporary, 0o700)?;
        fs::rename(&temporary, &executable).context("publish adapter executable snapshot")?;
        validate_private_file(&executable, 0o700, "recovery executable")?;
    }

    let generation = generation_name(
        config.options.listen,
        build_id,
        &config.service_config_fingerprint,
    );
    let manifest = RecoveryManifest {
        generation: generation.clone(),
        protocol_version: crate::ADAPTER_PROTOCOL_VERSION,
        build_id: build_id.to_owned(),
        listen: config.options.listen,
        model: config.options.model.clone(),
        arguments: daemon_arguments(&config.options)
            .into_iter()
            .map(|argument| {
                argument
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("daemon recovery argument must be UTF-8"))
            })
            .collect::<Result<Vec<_>>>()?,
        codex_config_fingerprint: config.codex_config_fingerprint.clone(),
        service_config_fingerprint: config.service_config_fingerprint.clone(),
    };
    let path = root.join(manifest_file_name(&generation));
    if path.exists() {
        let existing = read_manifest(&path)?;
        ensure!(
            existing == manifest,
            "immutable recovery generation changed"
        );
    } else {
        publish_manifest(&path, &manifest)?;
    }
    cleanup(&root, config.options.listen, &generation)?;
    Ok(path)
}

pub(super) fn validate(config: &ServiceConfig, generation: &str) -> Result<ValidatedRecovery> {
    ensure!(safe_component(generation), "invalid recovery generation");
    let root = recovery_root(config)?;
    validate_private_directory(&root, "recovery directory")?;
    let manifest_path = root.join(manifest_file_name(generation));
    validate_private_file(&manifest_path, 0o600, "recovery manifest")?;
    let manifest = read_manifest(&manifest_path)?;
    ensure!(
        manifest.generation == generation,
        "recovery generation mismatch"
    );
    ensure!(
        manifest.listen == config.options.listen,
        "recovery listener mismatch"
    );
    ensure!(
        manifest.protocol_version > 0
            && manifest.protocol_version <= crate::ADAPTER_PROTOCOL_VERSION,
        "unsupported recovery protocol"
    );
    ensure!(
        safe_component(&manifest.build_id),
        "invalid recovery build ID"
    );
    ensure!(
        generation_name(
            manifest.listen,
            &manifest.build_id,
            &manifest.service_config_fingerprint
        ) == generation,
        "recovery manifest identity mismatch"
    );
    validate_arguments(&manifest)?;
    let build_directory = root.join(&manifest.build_id);
    validate_private_directory(&build_directory, "recovery generation directory")?;
    let executable = build_directory.join(EXECUTABLE_NAME);
    validate_private_file(&executable, 0o700, "recovery executable")?;
    Ok(ValidatedRecovery {
        generation: generation.to_owned(),
        manifest_path,
        executable,
        arguments: manifest.arguments.into_iter().map(Into::into).collect(),
        protocol_version: manifest.protocol_version,
        build_id: manifest.build_id,
        model: manifest.model,
        codex_config_fingerprint: manifest.codex_config_fingerprint,
        service_config_fingerprint: manifest.service_config_fingerprint,
    })
}

pub(super) fn generation_from_environment() -> Option<String> {
    let path = PathBuf::from(std::env::var_os(super::RECOVERY_MANIFEST_ENV)?);
    generation_from_path(&path)
}

pub(super) fn generation_from_path(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix(MANIFEST_PREFIX)?
        .strip_suffix(MANIFEST_SUFFIX)
        .filter(|generation| safe_component(generation))
        .map(str::to_owned)
}

pub(super) fn generation_name(
    listen: SocketAddr,
    build_id: &str,
    service_fingerprint: &str,
) -> String {
    let listener = listen
        .to_string()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("v1-{listener}-{build_id}-{service_fingerprint}")
}

fn validate_arguments(manifest: &RecoveryManifest) -> Result<()> {
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

fn publish_manifest(path: &Path, manifest: &RecoveryManifest) -> Result<()> {
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

fn cleanup(root: &Path, listen: SocketAddr, current_generation: &str) -> Result<()> {
    let mut matching = manifests(root)
        .into_iter()
        .filter(|(_, manifest, _)| manifest.listen == listen)
        .collect::<Vec<_>>();
    matching.sort_by_key(|(_, _, modified)| std::cmp::Reverse(*modified));
    let mut retained = HashSet::from([current_generation.to_owned()]);
    for (_, manifest, _) in &matching {
        if retained.len() < RETAINED_GENERATIONS_PER_LISTENER {
            retained.insert(manifest.generation.clone());
        }
    }
    for (path, manifest, _) in matching {
        if !retained.contains(&manifest.generation) {
            fs::remove_file(path).context("prune old recovery manifest")?;
        }
    }
    let referenced = manifests(root)
        .into_iter()
        .map(|(_, manifest, _)| manifest.build_id)
        .collect::<HashSet<_>>();
    for entry in fs::read_dir(root).context("list recovery generations")? {
        let entry = entry.context("read recovery generation entry")?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !safe_component(&name) || referenced.contains(&name) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(entry.path()).context("prune unused recovery executable")?;
        }
    }
    Ok(())
}

fn manifests(root: &Path) -> Vec<(PathBuf, RecoveryManifest, std::time::SystemTime)> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(manifest_entry)
        .collect()
}

fn manifest_entry(
    entry: fs::DirEntry,
) -> Option<(PathBuf, RecoveryManifest, std::time::SystemTime)> {
    let path = entry.path();
    let name = path.file_name()?.to_str()?;
    if !name.starts_with(MANIFEST_PREFIX) || !name.ends_with(MANIFEST_SUFFIX) {
        return None;
    }
    validate_private_file(&path, 0o600, "recovery manifest").ok()?;
    let manifest = read_manifest(&path).ok()?;
    let modified = entry.metadata().ok()?.modified().ok()?;
    Some((path, manifest, modified))
}

fn manifest_file_name(generation: &str) -> String {
    format!("{MANIFEST_PREFIX}{generation}{MANIFEST_SUFFIX}")
}

fn read_manifest(path: &Path) -> Result<RecoveryManifest> {
    serde_json::from_slice(&fs::read(path).context("read adapter recovery manifest")?)
        .context("decode adapter recovery manifest")
}

fn recovery_root(config: &ServiceConfig) -> Result<PathBuf> {
    Ok(config
        .log_path
        .parent()
        .context("adapter log has no parent")?
        .join("recovery"))
}

fn ensure_recovery_root(config: &ServiceConfig) -> Result<PathBuf> {
    let root = recovery_root(config)?;
    ensure_private_directory(&root)?;
    Ok(root)
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    if path.exists() {
        reject_symlink_and_wrong_owner(path, "recovery directory")?;
        ensure!(path.is_dir(), "recovery path is not a directory");
    } else {
        fs::create_dir_all(path).context("create adapter recovery directory")?;
    }
    set_private_permissions(path, 0o700)?;
    validate_private_directory(path, "recovery directory")
}

fn validate_private_directory(path: &Path, label: &str) -> Result<()> {
    reject_symlink_and_wrong_owner(path, label)?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(metadata.is_dir(), "{label} is not a directory");
    validate_mode(&metadata, 0o700, label)
}

fn validate_private_file(path: &Path, mode: u32, label: &str) -> Result<()> {
    reject_symlink_and_wrong_owner(path, label)?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(metadata.is_file(), "{label} is not a regular file");
    validate_mode(&metadata, mode, label)
}

fn reject_symlink_and_wrong_owner(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect {label} {}", path.display()))?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "{label} must not be a symlink"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "{label} is not owned by the current user"
        );
    }
    Ok(())
}

#[cfg(unix)]
fn validate_mode(metadata: &fs::Metadata, expected: u32, label: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    ensure!(
        metadata.permissions().mode() & 0o777 == expected,
        "{label} permissions must be {expected:o}"
    );
    Ok(())
}

#[cfg(not(unix))]
fn validate_mode(_metadata: &fs::Metadata, _expected: u32, _label: &str) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("secure adapter recovery path {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
#[path = "recovery_manifest_tests.rs"]
mod tests;
