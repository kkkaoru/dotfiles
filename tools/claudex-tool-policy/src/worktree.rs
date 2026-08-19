use crate::deny;
use crate::env::nonempty_str;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

const WORKTREE_MARKER: &str = "/.claude/worktrees/";

fn isolated_worktree_cwd(payload: &Map<String, Value>) -> Option<(&str, &str)> {
    let cwd = nonempty_str(payload.get("cwd"))?;
    let (repo, rest) = cwd.split_once(WORKTREE_MARKER)?;
    if repo.is_empty() || rest.is_empty() {
        return None;
    }
    Some((repo, cwd))
}

fn target_path(path: &str, repo: &str, home: &Path) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        return home.join(rest);
    }
    if path == "~" {
        return home.to_path_buf();
    }
    let raw = PathBuf::from(path);
    if raw.is_absolute() {
        return raw;
    }
    PathBuf::from(format!("{repo}/{path}"))
}

fn is_inside_worktree(target: &Path, worktree: &Path) -> bool {
    target == worktree || target.starts_with(worktree)
}

fn deny_outside(path: &str, target: &Path, cwd: &str) -> Value {
    deny(
        "PreToolUse",
        &format!(
            "Write target `{path}` resolves to `{}` outside this SubAgent worktree `{cwd}`. \
             Claude Code worktree isolation rejects that as `Error writing file` with no lock \
             holder. Rewrite the path so it is inside `{cwd}`.",
            target.display()
        ),
    )
}

/// Deny mutating tools that Claude Code isolated worktrees will reject as a
/// bare `Error writing file`.
pub(crate) fn deny_outside_isolated_worktree(
    payload: &Map<String, Value>,
    paths: &[String],
    home: &Path,
) -> Option<Value> {
    let (repo, cwd) = isolated_worktree_cwd(payload)?;
    let worktree = Path::new(cwd);
    let path = paths.iter().find(|path| {
        let target = target_path(path, repo, home);
        !is_inside_worktree(&target, worktree)
    })?;
    Some(deny_outside(path, &target_path(path, repo, home), cwd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    fn payload(cwd: &str) -> Map<String, Value> {
        json!({"cwd": cwd}).as_object().cloned().unwrap()
    }

    #[test]
    fn relative_repo_path_is_outside_worktree() {
        let denied = deny_outside_isolated_worktree(
            &payload("/Users/me/repo/.claude/worktrees/agent-a"),
            &["scripts/install.mjs".to_owned()],
            Path::new("/Users/me"),
        )
        .unwrap();
        let reason = denied["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap();
        assert!(reason.contains("Error writing file"));
        assert!(reason.contains(".claude/worktrees/agent-a"));
        assert!(!reason.contains("locked by"));
        assert!(
            denied["reason"]
                .as_str()
                .unwrap()
                .contains("Error writing file")
        );
    }

    #[test]
    fn worktree_absolute_path_is_allowed() {
        let denied = deny_outside_isolated_worktree(
            &payload("/Users/me/repo/.claude/worktrees/agent-a"),
            &["/Users/me/repo/.claude/worktrees/agent-a/scripts/install.mjs".to_owned()],
            Path::new("/Users/me"),
        );
        assert!(denied.is_none());
    }

    #[test]
    fn repo_relative_worktree_path_is_allowed() {
        let denied = deny_outside_isolated_worktree(
            &payload("/Users/me/repo/.claude/worktrees/agent-a"),
            &[".claude/worktrees/agent-a/scripts/install.mjs".to_owned()],
            Path::new("/Users/me"),
        );
        assert!(denied.is_none());
    }

    #[test]
    fn non_worktree_cwd_is_ignored() {
        let denied = deny_outside_isolated_worktree(
            &payload("/Users/me/repo"),
            &["scripts/install.mjs".to_owned()],
            Path::new("/Users/me"),
        );
        assert!(denied.is_none());
    }

    #[test]
    fn incomplete_worktree_cwd_is_ignored() {
        let denied = deny_outside_isolated_worktree(
            &payload("/Users/me/repo/.claude/worktrees/"),
            &["scripts/install.mjs".to_owned()],
            Path::new("/Users/me"),
        );
        assert!(denied.is_none());
    }

    #[test]
    fn home_relative_path_outside_worktree_is_denied() {
        let denied = deny_outside_isolated_worktree(
            &payload("/Users/me/repo/.claude/worktrees/agent-a"),
            &["~/other.rs".to_owned()],
            Path::new("/Users/me"),
        )
        .unwrap();
        assert!(
            denied["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("Error writing file")
        );
    }

    #[test]
    fn home_path_outside_worktree_is_denied() {
        let denied = deny_outside_isolated_worktree(
            &payload("/Users/me/repo/.claude/worktrees/agent-a"),
            &["~".to_owned()],
            Path::new("/Users/me"),
        )
        .unwrap();
        assert!(
            denied["hookSpecificOutput"]["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("Error writing file")
        );
    }
}
