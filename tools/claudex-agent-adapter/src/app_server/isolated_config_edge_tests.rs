use std::path::Path;

use super::*;

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
fn production_isolated_home_keeps_a_real_source_codex_home() {
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
    let resolved = effective_source_home(&user_codex, &isolated);
    assert_eq!(resolved, user_codex);
}

#[test]
fn stub_auth_treats_unreadable_paths_as_stubs() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("auth.json")).unwrap();
    assert!(is_stub_auth(&root.path().join("auth.json")));
    assert!(is_stub_auth(Path::new("/no/such/claudex-auth.json")));
}

#[test]
fn prune_skips_symlinked_directories() {
    let store = tempfile::tempdir().unwrap();
    std::fs::write(store.path().join("logs_2.sqlite"), "keep-via-symlink-skip").unwrap();
    let root = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(store.path(), root.path().join("linked-runtime")).unwrap();
    prune_runtime_logs(root.path());
    assert!(store.path().join("logs_2.sqlite").exists());
}

#[test]
fn append_providers_skips_duplicate_sections_and_unreadable_files() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("config.toml"),
        "[model_providers.fugu]\nname = \"first\"\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("fugu.config.toml"),
        "[model_providers.fugu]\nname = \"second\"\n[other]\nkeep = false\n",
    )
    .unwrap();
    let mut config = String::new();
    append_model_providers(root.path(), &mut config).unwrap();
    assert_eq!(config.matches("[model_providers.fugu]").count(), 1);
    assert!(!config.contains("[other]"));
}
