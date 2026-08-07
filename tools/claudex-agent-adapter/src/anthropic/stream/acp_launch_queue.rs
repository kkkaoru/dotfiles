//! Cursor ACP often reports MCP tool cards with empty `rawInput`, while the
//! injected `claudex-launch` MCP server still receives the real Agent/Task args.
//! `mcp-claudex-launch` appends those args to a local queue; we consume them here.

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

const QUEUE_MAX_AGE: Duration = Duration::from_secs(120);

pub(super) fn queue_path() -> PathBuf {
    env::var_os("CLAUDEX_LAUNCH_QUEUE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".cache/claudex/launch-queue.jsonl")
        })
}

/// Read the oldest fresh Agent/Task launch args without removing them.
pub(super) fn peek_pending_launch_arguments() -> Option<Value> {
    peek_pending_launch_arguments_from(&queue_path(), now_secs())
}

fn peek_pending_launch_arguments_from(path: &Path, now: f64) -> Option<Value> {
    read_entries(path)
        .into_iter()
        .find_map(|entry| launch_args_from_entry(&entry, now))
}

/// Pop the oldest fresh Agent/Task launch args written by `mcp-claudex-launch`.
pub(super) fn take_pending_launch_arguments() -> Option<Value> {
    let path = queue_path();
    take_pending_launch_arguments_from(&path, now_secs())
}

fn take_pending_launch_arguments_from(path: &Path, now: f64) -> Option<Value> {
    let entries = read_entries(path);
    let mut taken = None;
    let mut kept = Vec::new();
    for entry in entries {
        if taken.is_none()
            && let Some(arguments) = launch_args_from_entry(&entry, now)
        {
            taken = Some(arguments);
            continue;
        }
        let ts = entry.get("ts").and_then(Value::as_f64).unwrap_or(0.0);
        if now - ts <= QUEUE_MAX_AGE.as_secs_f64() {
            kept.push(entry);
        }
    }
    rewrite_queue(path, &kept);
    taken
}

fn read_entries(path: &Path) -> Vec<Value> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            (!line.is_empty())
                .then(|| serde_json::from_str::<Value>(line).ok())
                .flatten()
        })
        .collect()
}

fn launch_args_from_entry(entry: &Value, now: f64) -> Option<Value> {
    let ts = entry.get("ts").and_then(Value::as_f64).unwrap_or(0.0);
    if now - ts > QUEUE_MAX_AGE.as_secs_f64() {
        return None;
    }
    let name = entry.get("name").and_then(Value::as_str).unwrap_or("Agent");
    if !(name.eq_ignore_ascii_case("agent") || name.eq_ignore_ascii_case("task")) {
        return None;
    }
    let mut arguments = entry.get("arguments").cloned().unwrap_or_else(|| json!({}));
    if let Some(object) = arguments.as_object_mut() {
        object
            .entry("_toolName".to_owned())
            .or_insert_with(|| json!(name));
    }
    Some(arguments)
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn rewrite_queue(path: &Path, entries: &[Value]) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if entries.is_empty() {
        let _ = fs::write(path, "");
        return;
    }
    let body = entries
        .iter()
        .filter_map(|entry| serde_json::to_string(entry).ok())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let _ = fs::write(path, body);
}

#[cfg(test)]
#[path = "acp_launch_queue_tests.rs"]
mod tests;
