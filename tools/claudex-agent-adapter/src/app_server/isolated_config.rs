use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;

use super::codex_config::provider_config_files;

pub(super) fn append_model_providers(source_home: &Path, config: &mut String) -> Result<()> {
    let sources = provider_config_files(source_home)?;

    let mut copied_sections = HashSet::new();
    for source in sources {
        let Ok(contents) = std::fs::read_to_string(source) else {
            continue;
        };
        append_sections(&contents, config, &mut copied_sections);
    }
    Ok(())
}

pub(super) fn append_model_catalog(source_home: &Path, config: &mut String) -> Result<()> {
    for source in provider_config_files(source_home)? {
        let Ok(contents) = std::fs::read_to_string(&source) else {
            continue;
        };
        if let Some(value) = catalog_json_assignment(&contents) {
            config.push_str("model_catalog_json = ");
            config.push_str(&value);
            config.push('\n');
            return Ok(());
        }
    }
    Ok(())
}

/// Drop Codex sqlite logs in the isolated home. A multi-GB `logs_2.sqlite`
/// makes `initialize` miss the existing 8s budget without any extra timeout.
pub(super) fn prune_runtime_logs(isolated: &Path) {
    let Ok(entries) = std::fs::read_dir(isolated) else {
        return;
    };
    for entry in entries.flatten() {
        if is_runtime_log_db(&entry.file_name().to_string_lossy()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn catalog_json_assignment(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let trimmed = line.trim();
        let rest = trimmed.strip_prefix("model_catalog_json")?.trim();
        let value = rest.strip_prefix('=')?.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn is_runtime_log_db(name: &str) -> bool {
    let Some(stem) = name
        .strip_suffix(".sqlite-wal")
        .or_else(|| name.strip_suffix(".sqlite-shm"))
        .or_else(|| name.strip_suffix(".sqlite"))
    else {
        return false;
    };
    stem.starts_with("logs_")
}

fn append_sections(contents: &str, config: &mut String, copied_sections: &mut HashSet<String>) {
    let mut copying = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        copying = next_copying_state(trimmed, copying, copied_sections);
        if copying {
            config.push_str(line);
            config.push('\n');
        }
    }
}

fn next_copying_state(line: &str, current: bool, copied_sections: &mut HashSet<String>) -> bool {
    if !line.starts_with('[') {
        return current;
    }
    let provider_section = line == "[model_providers]" || line.starts_with("[model_providers.");
    provider_section && copied_sections.insert(line.to_owned())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn copies_the_first_catalog_assignment() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("config.toml"),
            "model_catalog_json = \"~/.codex/fugu.json\"\n",
        )
        .unwrap();
        let mut config = String::new();
        append_model_catalog(root.path(), &mut config).unwrap();
        assert_eq!(config, "model_catalog_json = \"~/.codex/fugu.json\"\n");
    }

    #[test]
    fn skips_empty_catalog_and_reads_a_profile() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("config.toml"), "model_catalog_json =\n").unwrap();
        std::fs::write(
            root.path().join("fugu.config.toml"),
            "model_catalog_json = \"/abs/fugu.json\"\n",
        )
        .unwrap();
        let mut config = String::new();
        append_model_catalog(root.path(), &mut config).unwrap();
        assert_eq!(config, "model_catalog_json = \"/abs/fugu.json\"\n");
    }

    #[test]
    fn leaves_config_unchanged_without_a_catalog() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("config.toml"), "[model_providers]\n").unwrap();
        let mut config = String::from("keep\n");
        append_model_catalog(root.path(), &mut config).unwrap();
        assert_eq!(config, "keep\n");
    }

    #[test]
    fn catalog_copy_reports_an_unreadable_source_home() {
        let missing = Path::new("/no/such/codex-source-home-for-catalog");
        assert!(append_model_catalog(missing, &mut String::new()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn skips_unreadable_provider_files_when_copying_catalog() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let blocked = root.path().join("config.toml");
        std::fs::write(&blocked, "model_catalog_json = \"secret\"\n").unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        std::fs::write(
            root.path().join("fugu.config.toml"),
            "model_catalog_json = \"ok.json\"\n",
        )
        .unwrap();
        let mut config = String::new();
        let result = append_model_catalog(root.path(), &mut config);
        let _ = std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o644));
        result.unwrap();
        assert_eq!(config, "model_catalog_json = \"ok.json\"\n");
    }

    #[test]
    fn prunes_codex_runtime_logs_and_keeps_other_files() {
        let root = tempfile::tempdir().unwrap();
        let isolated = root.path();
        std::fs::write(isolated.join("logs_2.sqlite"), "db").unwrap();
        std::fs::write(isolated.join("logs_2.sqlite-wal"), "wal").unwrap();
        std::fs::write(isolated.join("logs_2.sqlite-shm"), "shm").unwrap();
        std::fs::write(isolated.join("auth.json"), "{}").unwrap();
        std::fs::write(isolated.join("notes.sqlite"), "keep").unwrap();
        prune_runtime_logs(isolated);
        assert!(!isolated.join("logs_2.sqlite").exists());
        assert!(!isolated.join("logs_2.sqlite-wal").exists());
        assert!(!isolated.join("logs_2.sqlite-shm").exists());
        assert!(isolated.join("auth.json").exists());
        assert!(isolated.join("notes.sqlite").exists());
    }

    #[test]
    fn prune_ignores_missing_or_non_directory_homes() {
        prune_runtime_logs(Path::new("/no/such/claudex-isolated-home"));
        let file = tempfile::NamedTempFile::new().unwrap();
        prune_runtime_logs(file.path());
    }

    #[test]
    fn prune_ignores_unremovable_runtime_log_paths() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("logs_2.sqlite")).unwrap();
        prune_runtime_logs(root.path());
        assert!(root.path().join("logs_2.sqlite").is_dir());
    }

    #[test]
    fn ignores_catalog_lines_without_an_assignment() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("config.toml"),
            "model_catalog_json \"missing-eq\"\n",
        )
        .unwrap();
        let mut config = String::from("keep\n");
        append_model_catalog(root.path(), &mut config).unwrap();
        assert_eq!(config, "keep\n");
    }

    #[test]
    fn classifies_runtime_log_names() {
        assert!(is_runtime_log_db("logs_2.sqlite"));
        assert!(is_runtime_log_db("logs_2.sqlite-wal"));
        assert!(is_runtime_log_db("logs_2.sqlite-shm"));
        assert!(!is_runtime_log_db("auth.json"));
        assert!(!is_runtime_log_db("notes.sqlite"));
        assert!(!is_runtime_log_db("logs_2.txt"));
    }
}
