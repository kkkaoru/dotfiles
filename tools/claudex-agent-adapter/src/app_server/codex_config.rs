use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

pub(crate) const CODEX_CONFIG_FINGERPRINT_ENV: &str = "CLAUDEX_CODEX_CONFIG_FINGERPRINT";

pub(crate) fn source_home() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(home).join(".codex")))
}

pub(crate) fn provider_config_files(source_home: &Path) -> Result<Vec<PathBuf>> {
    let mut sources = vec![source_home.join("config.toml")];
    let mut profiles = fs::read_dir(source_home)
        .with_context(|| format!("read Codex source home {}", source_home.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".config.toml") && name != "config.toml")
        })
        .collect::<Vec<_>>();
    profiles.sort();
    sources.extend(profiles);
    Ok(sources)
}

pub(crate) fn provider_config_fingerprint(source_home: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    source_home.hash(&mut hasher);
    match provider_config_files(source_home) {
        Ok(files) => {
            for file in files {
                hash_provider_file(&mut hasher, &file);
            }
        }
        Err(error) => {
            "unreadable-source-home".hash(&mut hasher);
            error.to_string().hash(&mut hasher);
        }
    }
    format!("{:016x}", hasher.finish())
}

fn hash_provider_file(hasher: &mut DefaultHasher, file: &Path) {
    file.hash(hasher);
    match fs::read(file) {
        Ok(contents) => contents.hash(hasher),
        Err(error) => {
            "unreadable".hash(hasher);
            error.to_string().hash(hasher);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_changes_when_a_provider_profile_changes() {
        let root = tempfile::tempdir().expect("Codex config fixture");
        fs::write(
            root.path().join("config.toml"),
            "[model_providers.sakana]\n",
        )
        .unwrap();
        fs::write(
            root.path().join("fugu.config.toml"),
            "model_provider = \"sakana\"\n",
        )
        .unwrap();

        let first = provider_config_fingerprint(root.path());
        fs::write(
            root.path().join("fugu.config.toml"),
            "model_provider = \"sakana\"\nstream_idle_timeout_ms = 7200000\n",
        )
        .unwrap();
        let second = provider_config_fingerprint(root.path());

        assert_ne!(first, second);
        assert_eq!(first.len(), 16);
        assert_eq!(second.len(), 16);
    }

    #[test]
    fn provider_files_are_sorted_and_exclude_non_profile_configuration() {
        let root = tempfile::tempdir().expect("Codex config fixture");
        fs::write(root.path().join("config.toml"), "base").unwrap();
        fs::write(root.path().join("z.config.toml"), "z").unwrap();
        fs::write(root.path().join("a.config.toml"), "a").unwrap();
        fs::write(root.path().join("notes.toml"), "ignored").unwrap();

        let files = provider_config_files(root.path()).expect("provider files");
        assert_eq!(
            files,
            vec![
                root.path().join("config.toml"),
                root.path().join("a.config.toml"),
                root.path().join("z.config.toml")
            ]
        );
    }

    #[test]
    fn fingerprints_unreadable_sources_and_missing_provider_files() {
        let root = tempfile::tempdir().expect("Codex config fixture");
        let missing = root.path().join("missing");

        assert!(provider_config_files(&missing).is_err());
        let fingerprint = provider_config_fingerprint(&missing);
        assert_eq!(fingerprint.len(), 16);
        assert!(source_home().is_ok());
    }
}
