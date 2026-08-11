use std::{
    fs,
    net::SocketAddr,
    path::PathBuf,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{ServiceConfig, daemon_arguments::daemon_arguments};

mod fs_guard;
mod prune;
#[path = "recovery_manifest_validate.rs"]
mod validate;
pub(super) use validate::{generation_from_environment, generation_name, validate};
#[allow(unused_imports)] // exercised via recovery_manifest_tests / launcher_tests
pub(super) use validate::generation_from_path;
use fs_guard::{
    ensure_private_directory, set_private_permissions, validate_private_file,
};
#[cfg(test)]
use fs_guard::safe_component;
use prune::cleanup;
#[cfg(test)]
use prune::{manifest_entry, manifests};

pub(super) const EXECUTABLE_NAME: &str = "claudex-agent-adapter";
pub(super) const MANIFEST_PREFIX: &str = "manifest.";
pub(super) const MANIFEST_SUFFIX: &str = ".json";
const RETAINED_GENERATIONS_PER_LISTENER: usize = 2;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct RecoveryManifest {
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

#[path = "recovery_manifest_io.rs"]
mod io;
use io::{ensure_recovery_root, manifest_file_name, publish_manifest, read_manifest};
#[cfg(test)]
use io::validate_arguments;


#[cfg(test)]
#[path = "recovery_manifest_tests.rs"]
mod tests;
