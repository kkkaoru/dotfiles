use std::{
    env,
    ffi::OsString,
    path::PathBuf,
    process::Command,
};

/// PATH for long-lived adapter daemons and ACP child processes.
///
/// Launchers and GUI sessions often inherit a minimal PATH without user tool bins
/// such as `~/.bun/bin` (qwen) or Homebrew `node`. Configured ACP entries frequently
/// use `/usr/bin/env qwen ...`, and the qwen shim itself needs `node` on PATH.
pub(crate) fn tool_search_path() -> OsString {
    let mut parts = Vec::<PathBuf>::new();
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        parts.push(home.join(".bun/bin"));
        parts.push(home.join(".local/bin"));
        parts.push(home.join(".cargo/bin"));
    }
    parts.push(PathBuf::from("/opt/homebrew/bin"));
    parts.push(PathBuf::from("/usr/local/bin"));
    parts.push(PathBuf::from("/usr/bin"));
    parts.push(PathBuf::from("/bin"));
    if let Some(existing) = env::var_os("PATH") {
        for part in env::split_paths(&existing) {
            if !part.as_os_str().is_empty() && !parts.iter().any(|seen| seen == &part) {
                parts.push(part);
            }
        }
    }
    env::join_paths(parts.iter().map(|part| part.as_os_str())).unwrap_or_else(|_| {
        OsString::from("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin")
    })
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
}
