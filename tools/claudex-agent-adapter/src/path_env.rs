use std::{env, ffi::OsString, path::PathBuf, process::Command};

/// PATH for long-lived adapter daemons and ACP child processes.
///
/// Launchers and GUI sessions often inherit a minimal PATH without user tool bins
/// such as `~/.bun/bin` (qwen) or Homebrew `node`. Configured ACP entries frequently
/// use `/usr/bin/env qwen ...`, and the qwen shim itself needs `node` on PATH.
pub(crate) fn tool_search_path() -> OsString {
    tool_search_path_from(env::var_os("HOME"), env::var_os("PATH"))
}

fn tool_search_path_from(home: Option<OsString>, existing: Option<OsString>) -> OsString {
    let mut parts = Vec::<PathBuf>::new();
    if let Some(home) = home {
        let home = PathBuf::from(home);
        parts.push(home.join(".bun/bin"));
        parts.push(home.join(".local/bin"));
        parts.push(home.join(".cargo/bin"));
    }
    parts.push(PathBuf::from("/opt/homebrew/bin"));
    parts.push(PathBuf::from("/usr/local/bin"));
    parts.push(PathBuf::from("/usr/bin"));
    parts.push(PathBuf::from("/bin"));
    if let Some(existing) = existing {
        for part in env::split_paths(&existing) {
            if !part.as_os_str().is_empty() && !parts.iter().any(|seen| seen == &part) {
                parts.push(part);
            }
        }
    }
    env::join_paths(parts.iter().map(|part| part.as_os_str()))
        .unwrap_or_else(|_| OsString::from("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"))
}

/// Environment for a detached `serve` daemon started via nohup.
pub(crate) fn apply_daemon_env<'a>(command: &'a mut Command, token: &str) -> &'a mut Command {
    command
        .env("ANTHROPIC_AUTH_TOKEN", token)
        .env("PATH", tool_search_path())
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("ANTHROPIC_MODEL")
        .env_remove("CLAUDE_CODE_SUBAGENT_MODEL")
        .env_remove("CLAUDE_CODE_USE_BEDROCK")
        .env_remove("CLAUDE_CODE_USE_FOUNDRY")
        .env_remove("CLAUDE_CODE_USE_VERTEX")
        .env_remove("CLAUDEX_ADAPTER_LISTEN")
        .env_remove("CLAUDEX_BACKEND")
        .env_remove("CLAUDEX_CLAUDE_PROGRAM")
        .env_remove("CLAUDEX_MODEL")
        .env_remove("CLAUDEX_SUBSCRIPTION_MAX_PROCESSES")
        .env_remove("CLAUDEX_SUBSCRIPTION_TIMEOUT_MINUTES")
}

#[cfg(test)]
// Coverage gates measure production path construction; this inline module only contains tests.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn tool_search_path_includes_homebrew_and_user_bins() {
        let path = tool_search_path().to_string_lossy().into_owned();
        assert!(path.contains("/opt/homebrew/bin"));
        assert!(path.contains("/usr/local/bin"));
        if env::var_os("HOME").is_some() {
            assert!(path.contains("/.bun/bin"));
            assert!(path.contains("/.local/bin"));
            assert!(path.contains("/.cargo/bin"));
        }
    }

    #[test]
    fn handles_missing_home_path_and_duplicate_entries() {
        let without_environment = tool_search_path_from(None, None);
        assert!(without_environment.to_string_lossy().contains("/usr/bin"));
        let existing = env::join_paths([
            std::ffi::OsStr::new(""),
            std::ffi::OsStr::new("/usr/bin"),
            std::ffi::OsStr::new("/custom/bin"),
        ])
        .expect("fixture PATH");
        let path = tool_search_path_from(Some(OsString::from("/home/test")), Some(existing));
        let path = path.to_string_lossy();
        assert!(path.contains("/home/test/.bun/bin"));
        assert!(path.contains("/custom/bin"));
        assert_eq!(path.matches("/usr/bin").count(), 1);
    }
}
