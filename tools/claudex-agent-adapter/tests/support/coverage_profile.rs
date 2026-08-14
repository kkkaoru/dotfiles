//! Coverage-profile containment for force-stopped Rust fixture children.
//!
//! `cargo llvm-cov` merges every profile beneath its target directory. Some
//! integration fixtures deliberately terminate their provider or daemon child
//! with SIGKILL; an instrumented child can then leave a truncated `.profraw`.
//! Keep those child-only profiles in the owning test fixture instead. This
//! module is Unix-only because the integration suite itself uses Unix process
//! semantics. Callers must retain their unique temporary fixture root until
//! the wrapped child exits.

use std::{
    ffi::OsStr,
    fs,
    io::Write,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

const PROFILE_DIRECTORY: &str = "discarded-llvm-profiles";
static NEXT_WRAPPER: AtomicU64 = AtomicU64::new(0);
static STABLE_WRAPPER_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn wrapped_program(fixture_root: &Path, program: impl AsRef<OsStr>) -> PathBuf {
    let program = Path::new(program.as_ref());
    fs::create_dir_all(profile_directory(fixture_root)).expect("create fixture profile directory");
    let wrapper = fixture_root.join(format!(
        "fixture-profile-wrapper-{}",
        NEXT_WRAPPER.fetch_add(1, Ordering::Relaxed)
    ));
    write_wrapper(&wrapper, fixture_root, program);
    wrapper
}

/// Return one atomically-created wrapper per fixture root/program pair.
///
/// Launcher identity tests intentionally invoke `ensure` repeatedly; the
/// provider-program path must stay stable there or a healthy daemon would be
/// replaced merely because its test wrapper received a new name.
#[allow(dead_code)]
pub(crate) fn stable_wrapped_program(fixture_root: &Path, program: impl AsRef<OsStr>) -> PathBuf {
    let _guard = STABLE_WRAPPER_LOCK
        .lock()
        .expect("lock stable fixture profile wrapper creation");
    let program = Path::new(program.as_ref());
    fs::create_dir_all(profile_directory(fixture_root)).expect("create fixture profile directory");
    let wrapper = fixture_root.join(format!(
        "fixture-profile-stable-{}",
        hex_name(program.as_os_str())
    ));
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(wrapper.as_path())
    {
        Ok(file) => write_wrapper_file(file, &wrapper, fixture_root, program),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => panic!(
            "create stable fixture profile wrapper {}: {error}",
            wrapper.display()
        ),
    }
    wrapper
}

fn write_wrapper(wrapper: &Path, fixture_root: &Path, program: &Path) {
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(wrapper)
        .expect("create unique fixture profile wrapper");
    write_wrapper_file(file, wrapper, fixture_root, program);
}

fn write_wrapper_file(mut file: fs::File, wrapper: &Path, fixture_root: &Path, program: &Path) {
    let mut script = b"#!/bin/sh\nexport LLVM_PROFILE_FILE=".to_vec();
    script.extend(shell_quote(profile_pattern(fixture_root).as_os_str()));
    script.extend(b"\nexec ");
    script.extend(shell_quote(program.as_os_str()));
    script.extend(b" \"$@\"\n");
    file.write_all(&script)
        .expect("write fixture profile wrapper");
    drop(file);
    let mut permissions = fs::metadata(wrapper)
        .expect("read fixture profile wrapper metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(wrapper, permissions).expect("make fixture profile wrapper executable");
}

#[allow(dead_code)]
pub(crate) fn wrapped_program_string(fixture_root: &Path, program: impl AsRef<OsStr>) -> String {
    wrapped_program(fixture_root, program)
        .into_os_string()
        .into_string()
        .expect("fixture wrapper path must be UTF-8 for an ACP launch configuration")
}

fn profile_directory(fixture_root: &Path) -> PathBuf {
    fixture_root.join(PROFILE_DIRECTORY)
}

fn profile_pattern(fixture_root: &Path) -> PathBuf {
    profile_directory(fixture_root).join("fixture-%m-%p.profraw")
}

fn shell_quote(value: &OsStr) -> Vec<u8> {
    let mut quoted = Vec::with_capacity(value.as_bytes().len() + 2);
    quoted.push(b'\'');
    for byte in value.as_bytes() {
        if *byte == b'\'' {
            quoted.extend(b"'\\''");
        } else {
            quoted.push(*byte);
        }
    }
    quoted.push(b'\'');
    quoted
}

fn hex_name(value: &OsStr) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn child_override_is_isolated_from_the_main_profile_pattern() {
        let root = tempfile::tempdir().expect("create profile fixture");
        let target = root.path().join("print profile's value");
        fs::write(&target, "#!/bin/sh\nprintf '%s' \"$LLVM_PROFILE_FILE\"\n")
            .expect("write profile target");
        let mut permissions = fs::metadata(&target)
            .expect("read profile target metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&target, permissions).expect("make profile target executable");

        let main_profile = std::env::var_os("LLVM_PROFILE_FILE");
        let wrapper = wrapped_program(root.path(), &target);
        let output = Command::new(wrapper)
            .env("LLVM_PROFILE_FILE", "main-coverage-%p.profraw")
            .output()
            .expect("run wrapped fixture child");

        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).expect("fixture profile output"),
            profile_pattern(root.path()).display().to_string()
        );
        assert_eq!(std::env::var_os("LLVM_PROFILE_FILE"), main_profile);
        assert!(
            !profile_pattern(root.path())
                .display()
                .to_string()
                .contains("main-coverage")
        );
    }

    #[test]
    fn concurrent_wrappers_are_unique_for_one_fixture_root() {
        let root = tempfile::tempdir().expect("create concurrent wrapper fixture");
        let target = root.path().join("target");
        let wrappers = std::thread::scope(|scope| {
            (0..8)
                .map(|_| scope.spawn(|| wrapped_program(root.path(), &target)))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|task| task.join().expect("create wrapper concurrently"))
                .collect::<Vec<_>>()
        });

        let unique = wrappers.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), wrappers.len());
        assert!(wrappers.iter().all(|wrapper| wrapper.is_file()));
    }
}
