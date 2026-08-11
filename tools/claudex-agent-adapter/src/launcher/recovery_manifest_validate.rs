use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Result, ensure};

use super::{
    EXECUTABLE_NAME, MANIFEST_PREFIX, MANIFEST_SUFFIX, ServiceConfig, ValidatedRecovery,
    fs_guard::{safe_component, validate_private_directory, validate_private_file},
    io::{manifest_file_name, read_manifest, recovery_root, validate_arguments},
};

pub(in crate::launcher) fn validate(
    config: &ServiceConfig,
    generation: &str,
) -> Result<ValidatedRecovery> {
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

pub(in crate::launcher) fn generation_from_environment() -> Option<String> {
    let path = PathBuf::from(std::env::var_os(crate::launcher::RECOVERY_MANIFEST_ENV)?);
    generation_from_path(&path)
}

pub(in crate::launcher) fn generation_from_path(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix(MANIFEST_PREFIX)?
        .strip_suffix(MANIFEST_SUFFIX)
        .filter(|generation| safe_component(generation))
        .map(str::to_owned)
}

pub(in crate::launcher) fn generation_name(
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
