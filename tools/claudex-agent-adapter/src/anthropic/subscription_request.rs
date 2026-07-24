use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{MessagesRequest, content::system_text};

pub(super) fn subscription_request_prompt(request: &MessagesRequest) -> String {
    format!(
        "Act as the requested Claude Code model. Follow the system instructions and complete the conversation below. Use only the enabled tools when needed. When delegation is requested and selected_workers are present, invoke the selected Agent or Task directly as the first tool call; do not perform task-list bookkeeping first. Treat current routing context as authoritative over stale model-policy memory. When a Task or Agent tool schema lacks claudex_model or claudex_effort, put each routed value at the start of its prompt as an exact `claudex_model: <model>` or `claudex_effort: <effort>` line. Never put a gpt or grok ID in the native model field.\n\nSystem:\n{}\n\nMessages:\n{}",
        system_text(&request.system),
        serde_json::to_string(&request.messages).unwrap_or_default()
    )
}

pub(super) fn requested_tools(tools: &[Value], omit_task_bookkeeping: bool) -> Vec<String> {
    let mut selected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for name in tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .filter(|name| {
            !omit_task_bookkeeping
                || !matches!(*name, "TaskCreate" | "TaskUpdate" | "TaskList" | "TaskGet")
        })
    {
        if seen.insert(name) {
            selected.push(name.to_owned());
        }
    }
    selected
}

pub(super) fn subscription_request_cwd(request: &MessagesRequest) -> Option<PathBuf> {
    cwd_from_system(&system_text(&request.system))
}

pub(super) fn cwd_from_system(system: &str) -> Option<PathBuf> {
    system.lines().find_map(|line| {
        let line = line.trim().strip_prefix("- ").unwrap_or(line.trim());
        let raw_path = [
            "Primary working directory: ",
            "Working directory: ",
            "CWD: ",
        ]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))?;
        let path = Path::new(raw_path.trim());
        if !path.is_absolute() {
            return None;
        }
        let canonical = std::fs::canonicalize(path).ok()?;
        canonical.is_dir().then_some(canonical)
    })
}
