use std::{
    collections::HashSet,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use super::fs_guard::{safe_component, validate_private_file};
use super::{
    MANIFEST_PREFIX, MANIFEST_SUFFIX, RETAINED_GENERATIONS_PER_LISTENER, RecoveryManifest,
    read_manifest,
};

pub(super) fn cleanup(root: &Path, listen: SocketAddr, current_generation: &str) -> Result<()> {
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

pub(super) fn manifests(root: &Path) -> Vec<(PathBuf, RecoveryManifest, std::time::SystemTime)> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(manifest_entry)
        .collect()
}

pub(super) fn manifest_entry(
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
