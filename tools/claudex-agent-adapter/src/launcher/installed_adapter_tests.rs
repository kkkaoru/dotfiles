use super::*;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    home: Option<std::ffi::OsString>,
    cargo_home: Option<std::ffi::OsString>,
    adapter: Option<std::ffi::OsString>,
    notify: Option<std::ffi::OsString>,
    recovery: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn push() -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self {
            _lock: lock,
            home: env::var_os("HOME"),
            cargo_home: env::var_os("CARGO_HOME"),
            adapter: env::var_os(ADAPTER_EXECUTABLE_ENV),
            notify: env::var_os(NOTIFY_IN_PROCESS_ENV),
            recovery: env::var_os(super::super::RECOVERY_MANIFEST_ENV),
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        restore("HOME", self.home.as_ref());
        restore("CARGO_HOME", self.cargo_home.as_ref());
        restore(ADAPTER_EXECUTABLE_ENV, self.adapter.as_ref());
        restore(NOTIFY_IN_PROCESS_ENV, self.notify.as_ref());
        restore(
            super::super::RECOVERY_MANIFEST_ENV,
            self.recovery.as_ref(),
        );
    }
}

fn restore(key: &str, value: Option<&std::ffi::OsString>) {
    match value {
        Some(value) => unsafe { env::set_var(key, value) },
        None => unsafe { env::remove_var(key) },
    }
}

#[test]
fn unify_promotes_real_local_binary_and_relinks_symlink() {
    let _guard = EnvGuard::push();
    let root = tempfile::tempdir().expect("home");
    let home = root.path();
    let cargo_bin = home.join(".cargo/bin");
    let local_bin = home.join(".local/bin");
    fs::create_dir_all(&cargo_bin).unwrap();
    fs::create_dir_all(&local_bin).unwrap();

    let local = local_bin.join("claudex-agent-adapter");
    let cargo = cargo_bin.join("claudex-agent-adapter");
    fs::write(&local, b"fresh-local").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&local, fs::Permissions::from_mode(0o755)).unwrap();
    }
    unsafe {
        env::set_var("HOME", home);
        env::set_var("CARGO_HOME", home.join(".cargo"));
        env::remove_var(ADAPTER_EXECUTABLE_ENV);
        env::remove_var(NOTIFY_IN_PROCESS_ENV);
        env::remove_var(super::super::RECOVERY_MANIFEST_ENV);
    }

    let resolved = unify_install_paths().expect("unify");
    assert_eq!(resolved, cargo);
    assert_eq!(fs::read(&cargo).unwrap(), b"fresh-local");
    assert!(local.is_symlink());
    assert_eq!(fs::read_link(&local).unwrap(), cargo);
    assert_eq!(
        resolve_service_executable(local.clone()),
        cargo,
        "service spawn must use cargo-bin even when started via ~/.local/bin"
    );
}

#[test]
fn resolve_keeps_test_harness_and_env_override() {
    let _guard = EnvGuard::push();
    let root = tempfile::tempdir().expect("home");
    let override_path = root.path().join("override-adapter");
    fs::write(&override_path, b"x").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&override_path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    unsafe {
        env::set_var("HOME", root.path());
        env::set_var(ADAPTER_EXECUTABLE_ENV, &override_path);
        env::remove_var(NOTIFY_IN_PROCESS_ENV);
        env::remove_var(super::super::RECOVERY_MANIFEST_ENV);
    }
    assert_eq!(
        resolve_service_executable(PathBuf::from("/tmp/claudex-agent-adapter")),
        override_path
    );
    unsafe { env::remove_var(ADAPTER_EXECUTABLE_ENV) };
    assert_eq!(
        resolve_service_executable(PathBuf::from("/tmp/deps/claudex_agent_adapter-abc123")),
        PathBuf::from("/tmp/deps/claudex_agent_adapter-abc123"),
        "unit-test harness binaries must not be redirected"
    );
}

#[test]
fn notify_delegate_disabled_when_already_in_process() {
    let _guard = EnvGuard::push();
    let root = tempfile::tempdir().expect("home");
    unsafe {
        env::set_var("HOME", root.path());
        env::set_var(NOTIFY_IN_PROCESS_ENV, "1");
        env::remove_var(ADAPTER_EXECUTABLE_ENV);
    }
    assert!(notify_delegate_executable().is_none());
}

#[test]
fn notify_delegate_uses_adapter_executable_override() {
    let _guard = EnvGuard::push();
    let root = tempfile::tempdir().expect("home");
    let exe = root.path().join("notify-delegate");
    fs::write(&exe, b"stub").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    }
    unsafe {
        env::set_var("HOME", root.path());
        env::set_var(ADAPTER_EXECUTABLE_ENV, &exe);
        env::remove_var(NOTIFY_IN_PROCESS_ENV);
    }
    assert_eq!(notify_delegate_executable().as_deref(), Some(exe.as_path()));
}

#[test]
fn resolve_keeps_recovery_manifest_and_recovery_path_images() {
    let _guard = EnvGuard::push();
    let recovery = PathBuf::from("/tmp/recovery/claudex-agent-adapter");
    unsafe {
        env::remove_var(ADAPTER_EXECUTABLE_ENV);
        env::set_var(super::super::RECOVERY_MANIFEST_ENV, "/tmp/recovery/manifest.json");
    };
    assert_eq!(resolve_service_executable(recovery.clone()), recovery);
    unsafe { env::remove_var(super::super::RECOVERY_MANIFEST_ENV) };
    assert_eq!(
        resolve_service_executable(PathBuf::from("/opt/recovery/bin/claudex-agent-adapter")),
        PathBuf::from("/opt/recovery/bin/claudex-agent-adapter")
    );
}

#[test]
fn unify_skips_promote_when_local_is_already_symlink() {
    let _guard = EnvGuard::push();
    let root = tempfile::tempdir().expect("home");
    let home = root.path();
    let cargo_bin = home.join(".cargo/bin");
    let local_bin = home.join(".local/bin");
    fs::create_dir_all(&cargo_bin).unwrap();
    fs::create_dir_all(&local_bin).unwrap();
    let local = local_bin.join("claudex-agent-adapter");
    let cargo = cargo_bin.join("claudex-agent-adapter");
    fs::write(&cargo, b"canonical").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(&cargo, &local).unwrap();
    }
    unsafe {
        env::set_var("HOME", home);
        env::set_var("CARGO_HOME", home.join(".cargo"));
        env::remove_var(ADAPTER_EXECUTABLE_ENV);
    }
    let resolved = unify_install_paths().expect("unify");
    assert_eq!(resolved, cargo);
    assert_eq!(fs::read(&cargo).unwrap(), b"canonical");
    assert!(local.is_symlink());
}

#[test]
fn resolve_falls_back_to_current_when_unify_fails() {
    let _guard = EnvGuard::push();
    let current = PathBuf::from("/tmp/claudex-agent-adapter");
    unsafe {
        env::remove_var("HOME");
        env::remove_var("CARGO_HOME");
        env::remove_var(ADAPTER_EXECUTABLE_ENV);
        env::remove_var(super::super::RECOVERY_MANIFEST_ENV);
    }
    assert_eq!(resolve_service_executable(current.clone()), current);
}

#[test]
fn is_executable_rejects_missing_and_non_executable_files() {
    let root = tempfile::tempdir().expect("temp");
    let missing = root.path().join("missing");
    assert!(!is_executable(&missing));
    let file = root.path().join("plain");
    fs::write(&file, b"x").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_executable(&file));
    }
}
