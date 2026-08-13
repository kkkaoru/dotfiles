use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::codex_config::provider_config_files;

/// Prefer the user's real `~/.codex` when preparing the production isolated
/// home. Cargo tests that set `CODEX_HOME` to a stub (`auth.json` = `{}`)
/// otherwise copy that stub over `~/.cache/claudex/codex-home` and fugu
/// loses `[model_providers.sakana]`.
pub(super) fn effective_source_home(source_home: &Path, isolated: &Path) -> PathBuf {
    if !is_production_isolated(isolated) {
        return source_home.to_path_buf();
    }
    let Some(user_home) = user_codex_home() else {
        return source_home.to_path_buf();
    };
    if (paths_equal(source_home, isolated) || is_stub_auth(&source_home.join("auth.json")))
        && !is_stub_auth(&user_home.join("auth.json"))
    {
        return user_home;
    }
    source_home.to_path_buf()
}

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
/// Nested `app-server-runtime/*/logs_2.sqlite` copies are pruned too.
pub(super) fn prune_runtime_logs(isolated: &Path) {
    prune_runtime_logs_at(isolated, 0);
}

const MAX_PRUNE_DEPTH: u32 = 6;

fn prune_runtime_logs_at(dir: &Path, depth: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if is_runtime_log_db(&name.to_string_lossy()) {
            let _ = std::fs::remove_file(entry.path());
            continue;
        }
        if depth >= MAX_PRUNE_DEPTH {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() && !file_type.is_symlink() {
            prune_runtime_logs_at(&entry.path(), depth + 1);
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

fn user_codex_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex"))
}

fn production_isolated_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache/claudex/codex-home"))
}

fn is_production_isolated(isolated: &Path) -> bool {
    production_isolated_home().is_some_and(|prod| paths_equal(isolated, &prod))
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => left == right,
    }
}

fn is_stub_auth(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return true;
    };
    matches!(bytes.trim_ascii(), b"" | b"{}")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "isolated_config_edge_tests.rs"]
mod edge_tests;

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

    #[test]
    fn prunes_nested_app_server_runtime_logs() {
        let root = tempfile::tempdir().unwrap();
        let nested = root
            .path()
            .join("app-server-runtime/app-server-deadbeef/sqlite");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("logs_2.sqlite"), "nested").unwrap();
        std::fs::write(nested.join("logs_2.sqlite-wal"), "wal").unwrap();
        std::fs::write(root.path().join("auth.json"), "{}").unwrap();
        prune_runtime_logs(root.path());
        assert!(!nested.join("logs_2.sqlite").exists());
        assert!(!nested.join("logs_2.sqlite-wal").exists());
        assert!(root.path().join("auth.json").exists());
    }

    static HOME_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct HomeGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        home: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn push() -> Self {
            let lock = HOME_ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self {
                _lock: lock,
                home: std::env::var_os("HOME"),
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.home {
                Some(home) => unsafe { std::env::set_var("HOME", home) },
                None => unsafe { std::env::remove_var("HOME") },
            }
        }
    }

    #[test]
    fn production_isolated_home_rejects_stub_codex_home() {
        let _guard = HomeGuard::push();
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let user_codex = home.join(".codex");
        let isolated = home.join(".cache/claudex/codex-home");
        std::fs::create_dir_all(&user_codex).unwrap();
        std::fs::create_dir_all(&isolated).unwrap();
        std::fs::write(
            user_codex.join("auth.json"),
            r#"{"tokens":{"access":"real"}}"#,
        )
        .unwrap();
        let stub = root.path().join("stub-codex");
        std::fs::create_dir(&stub).unwrap();
        std::fs::write(stub.join("auth.json"), "{}").unwrap();
        unsafe {
            std::env::set_var("HOME", &home);
        }
        let resolved = effective_source_home(&stub, &isolated);
        assert_eq!(resolved, user_codex);
    }

    #[test]
    fn temp_isolated_home_keeps_the_given_stub_source() {
        let _guard = HomeGuard::push();
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(
            home.join(".codex/auth.json"),
            r#"{"tokens":{"access":"real"}}"#,
        )
        .unwrap();
        let stub = root.path().join("stub-codex");
        let isolated = root.path().join("temp-isolated");
        std::fs::create_dir(&stub).unwrap();
        std::fs::create_dir(&isolated).unwrap();
        std::fs::write(stub.join("auth.json"), "{}").unwrap();
        unsafe {
            std::env::set_var("HOME", &home);
        }
        let resolved = effective_source_home(&stub, &isolated);
        assert_eq!(resolved, stub);
    }

    #[test]
    fn production_isolated_home_redirects_when_source_is_the_isolated_path() {
        let _guard = HomeGuard::push();
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let user_codex = home.join(".codex");
        let isolated = home.join(".cache/claudex/codex-home");
        std::fs::create_dir_all(&user_codex).unwrap();
        std::fs::create_dir_all(&isolated).unwrap();
        std::fs::write(
            user_codex.join("auth.json"),
            r#"{"tokens":{"access":"real"}}"#,
        )
        .unwrap();
        std::fs::write(isolated.join("auth.json"), "{}").unwrap();
        unsafe {
            std::env::set_var("HOME", &home);
        }
        let resolved = effective_source_home(&isolated, &isolated);
        assert_eq!(resolved, user_codex);
    }
}
