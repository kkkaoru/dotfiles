use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

pub(crate) const HEADER_NAME: &str = "x-claudex-working-directory";

/// Resolve a directory the adapter process can spawn children from.
///
/// `std::env::current_dir()` fails with ENOENT when the daemon was started in a
/// directory that has since been deleted (hot-swap temp worktrees). Fall back
/// to `~/.cache/claudex` instead of failing the first Grok/ACP turn.
pub(crate) fn resolve_process_cwd(context: &str) -> Result<PathBuf> {
    resolve_cwd(std::env::current_dir(), context)
}

/// Move off a deleted or ephemeral install cwd so later `getcwd` calls succeed.
pub(crate) fn pin_process_cwd() -> Result<PathBuf> {
    match std::env::current_dir() {
        Ok(cwd) if cwd.is_dir() && !is_ephemeral_install_cwd(&cwd) => Ok(cwd),
        _ => {
            let cwd = fallback_process_cwd()?;
            std::env::set_current_dir(&cwd)
                .with_context(|| format!("pin adapter working directory to {}", cwd.display()))?;
            Ok(cwd)
        }
    }
}

fn resolve_cwd(current: io::Result<PathBuf>, context: &str) -> Result<PathBuf> {
    if let Ok(cwd) = &current
        && cwd.is_dir()
    {
        return Ok(cwd.clone());
    }
    fallback_process_cwd().with_context(|| match current {
        Ok(cwd) => format!(
            "{context}: current directory {} is not a directory",
            cwd.display()
        ),
        Err(error) => format!("{context}: {error}"),
    })
}

fn fallback_process_cwd() -> Result<PathBuf> {
    if let Some(cache) = adapter_cache_dir() {
        std::fs::create_dir_all(&cache).ok();
        if let Ok(canonical) = cache.canonicalize()
            && canonical.is_dir()
        {
            return Ok(canonical);
        }
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && let Ok(canonical) = home.canonicalize()
        && canonical.is_dir()
    {
        return Ok(canonical);
    }
    let tmp = std::env::temp_dir();
    tmp.canonicalize()
        .ok()
        .filter(|path| path.is_dir())
        .with_context(|| format!("no usable fallback working directory ({})", tmp.display()))
}

fn adapter_cache_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache/claudex"))
}

fn is_ephemeral_install_cwd(cwd: &Path) -> bool {
    cwd.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|name| name.starts_with("claudex-temp-swap"))
    })
}

pub(crate) fn custom_headers(
    existing: Option<&OsStr>,
    cwd: &Path,
    disabled_subagent_models: Option<&str>,
) -> String {
    let mut headers = existing
        .map(|value| value.to_string_lossy())
        .into_iter()
        .flat_map(|value| {
            value
                .lines()
                .filter(|line| !reserved_header(line))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    headers.push(format!("{HEADER_NAME}: {}", encode(cwd)));
    if let Some(models) = disabled_subagent_models {
        headers.push(format!("{}: {models}", crate::subagent_policy::HEADER_NAME));
    }
    headers.join("\n")
}

pub(crate) fn encode(path: &Path) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::new();
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

pub(crate) fn decode(value: &str) -> Option<PathBuf> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let high = hex_value(*bytes.get(index + 1)?)?;
        let low = hex_value(*bytes.get(index + 2)?)?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded).ok().map(PathBuf::from)
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn reserved_header(line: &str) -> bool {
    line.split_once(':').is_some_and(|(name, _)| {
        let name = name.trim();
        name.eq_ignore_ascii_case(HEADER_NAME)
            || name.eq_ignore_ascii_case(crate::subagent_policy::HEADER_NAME)
    })
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn round_trips_visible_and_encoded_paths() {
        for path in ["/tmp/project", "/tmp/space and 日本語/%"] {
            assert_eq!(decode(&encode(Path::new(path))), Some(PathBuf::from(path)));
        }
    }

    #[test]
    fn merges_custom_headers_and_replaces_the_reserved_header() {
        let headers = custom_headers(
            Some(OsStr::new(
                "x-user-header: keep\nX-CLAUDEX-WORKING-DIRECTORY: forged\nX-CLAUDEX-DISABLED-SUBAGENT-MODELS: forged",
            )),
            Path::new("/tmp/project with space"),
            Some("gpt-5.6-sol,grok-4.6"),
        );
        assert_eq!(
            headers,
            "x-user-header: keep\nx-claudex-working-directory: /tmp/project%20with%20space\nx-claudex-disabled-subagent-models: gpt-5.6-sol,grok-4.6"
        );
        assert_eq!(
            custom_headers(None, Path::new("/tmp/plain"), None),
            "x-claudex-working-directory: /tmp/plain"
        );
    }

    #[test]
    fn rejects_malformed_percent_encoding_and_utf8() {
        for invalid in ["/tmp/%", "/tmp/%0", "/tmp/%GG", "/tmp/%FF"] {
            assert!(decode(invalid).is_none());
        }
    }

    #[test]
    fn keeps_an_existing_process_cwd() {
        let root = tempfile::tempdir().expect("cwd fixture");
        let cwd = resolve_cwd(Ok(root.path().to_path_buf()), "ctx").expect("keep cwd");
        assert_eq!(cwd, root.path());
    }

    #[test]
    fn falls_back_when_process_cwd_is_gone() {
        let cwd = resolve_cwd(
            Err(io::Error::from_raw_os_error(2)),
            "resolve Grok ACP working directory",
        )
        .expect("fallback cwd");
        assert!(cwd.is_dir(), "{}", cwd.display());
    }

    #[test]
    fn falls_back_when_process_cwd_is_a_file() {
        let root = tempfile::tempdir().expect("cwd fixture");
        let file = root.path().join("not-a-dir");
        std::fs::write(&file, b"").expect("write file cwd");
        let cwd = resolve_cwd(Ok(file), "resolve Grok ACP working directory").expect("fallback");
        assert!(cwd.is_dir(), "{}", cwd.display());
    }

    #[test]
    fn treats_temp_swap_paths_as_ephemeral() {
        assert!(is_ephemeral_install_cwd(Path::new(
            "/private/tmp/claudex-temp-swap-60b7db9"
        )));
        assert!(!is_ephemeral_install_cwd(Path::new(
            "/Users/kkk4oru/ghq/github.com/kkkaoru/dotfiles"
        )));
    }

    #[test]
    fn pin_process_cwd_moves_off_an_ephemeral_install_directory() {
        let original = std::env::current_dir().expect("original cwd");
        let tmp = tempfile::tempdir().expect("ephemeral fixture");
        let ephemeral = tmp.path().join("claudex-temp-swap-branch");
        std::fs::create_dir(&ephemeral).expect("create ephemeral cwd");
        std::env::set_current_dir(&ephemeral).expect("enter ephemeral cwd");
        let pinned = pin_process_cwd();
        let restore = std::env::set_current_dir(&original);
        let pinned = pinned.expect("pin off ephemeral cwd");
        restore.expect("restore original cwd");
        assert!(pinned.is_dir(), "{}", pinned.display());
        assert!(
            !is_ephemeral_install_cwd(&pinned),
            "pinned cwd must not remain on a temp-swap path: {}",
            pinned.display()
        );
    }

    #[test]
    fn decode_rejects_a_truncated_percent_sequence() {
        assert!(decode("%2").is_none());
        assert!(decode("%").is_none());
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
    fn fallback_process_cwd_uses_the_temp_dir_when_home_is_unset() {
        let _guard = HomeGuard::push();
        unsafe {
            std::env::remove_var("HOME");
        }
        let cwd = fallback_process_cwd().expect("fallback without HOME");
        assert!(cwd.is_dir(), "{}", cwd.display());
    }

    #[test]
    fn fallback_process_cwd_falls_through_home_when_the_cache_dir_cannot_be_created() {
        let _guard = HomeGuard::push();
        let root = tempfile::tempdir().expect("home fixture");
        // `HOME` points at a file, not a directory, so `create_dir_all` for
        // `$HOME/.cache/claudex` fails and its `canonicalize()` also fails.
        let fake_home = root.path().join("not-a-directory");
        std::fs::write(&fake_home, b"").expect("write fake HOME file");
        unsafe {
            std::env::set_var("HOME", &fake_home);
        }
        let cwd = fallback_process_cwd().expect("fallback past an uncreatable cache dir");
        // `HOME` itself canonicalizes but is not a directory either, so this
        // must fall all the way through to the temp-dir fallback.
        assert!(cwd.is_dir(), "{}", cwd.display());
        assert_ne!(cwd, fake_home);
    }

    #[test]
    fn fallback_process_cwd_skips_a_cache_dir_that_is_actually_a_file() {
        let _guard = HomeGuard::push();
        let root = tempfile::tempdir().expect("home fixture");
        let home = root.path().join("home");
        std::fs::create_dir_all(home.join(".cache")).expect("create .cache directory");
        // The leaf itself exists as a plain file, so `canonicalize()`
        // succeeds but `is_dir()` is false.
        std::fs::write(home.join(".cache/claudex"), b"").expect("occupy cache dir with a file");
        unsafe {
            std::env::set_var("HOME", &home);
        }
        let cwd = fallback_process_cwd().expect("fallback past a file-shaped cache dir");
        // Falls through to the HOME directory itself, which is a real dir.
        assert_eq!(cwd, home.canonicalize().expect("canonical home"));
    }
}
