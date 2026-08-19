use crate::deny;
use crate::env::nonempty_str;
use serde_json::{Map, Value};
use std::path::PathBuf;

mod acquire;
mod live;
mod store;

pub(crate) use acquire::{
    acquire_locks, release_agent_locks, release_paths, release_session_locks,
};

fn path_from_edit(edit: &Value) -> Option<String> {
    let object = edit.as_object()?;
    nonempty_str(object.get("file_path"))
        .or_else(|| nonempty_str(object.get("path")))
        .map(str::to_owned)
}

fn collect_edit_paths(tool_input: &Map<String, Value>, paths: &mut Vec<String>) {
    let Some(edits) = tool_input.get("edits").and_then(Value::as_array) else {
        return;
    };
    paths.extend(edits.iter().filter_map(path_from_edit));
}

pub(crate) fn tool_file_paths(_tool_name: &str, tool_input: &Map<String, Value>) -> Vec<String> {
    let mut paths = Vec::new();
    for key in ["file_path", "path", "notebook_path"] {
        paths.extend(nonempty_str(tool_input.get(key)).map(str::to_owned));
    }
    collect_edit_paths(tool_input, &mut paths);
    let mut seen = std::collections::HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    paths
}

fn resolve_absolute(path: &str, home: &std::path::Path) -> String {
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else if path == "~" {
        home.to_path_buf()
    } else {
        PathBuf::from(path)
    };
    std::fs::canonicalize(&expanded)
        .unwrap_or(expanded)
        .to_string_lossy()
        .into_owned()
}

fn deny_locked(absolute: &str, holder: &str) -> Value {
    deny(
        "PreToolUse",
        &format!(
            "File `{absolute}` is locked by SubAgent `{holder}`. Partition write scopes so parallel \
             workers do not edit the same path, or wait for that worker to finish before retrying."
        ),
    )
}

fn deny_lock_busy(absolute: &str) -> Value {
    deny(
        "PreToolUse",
        &format!(
            "File `{absolute}` lock is busy. Retry the write after the concurrent hook finishes."
        ),
    )
}

fn deny_lock_unsafe(absolute: &str) -> Value {
    deny(
        "PreToolUse",
        &format!(
            "File `{absolute}` lock is unsafe or unreadable. Remove the conflicting lock file."
        ),
    )
}
