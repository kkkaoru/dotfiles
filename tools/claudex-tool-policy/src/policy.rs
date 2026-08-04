use crate::allow;
use crate::deny;
use crate::env::{env_truthy, load_json_object, nonempty_str};
use crate::locks::{acquire_locks, release_agent_locks, release_paths, tool_file_paths};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::LazyLock;

pub(crate) static DENIED_MAIN_TOOLS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    HashSet::from([
        "Read",
        "Write",
        "Edit",
        "MultiEdit",
        "NotebookEdit",
        "Grep",
        "Glob",
        "LS",
        "WebSearch",
        "WebFetch",
    ])
});

pub(crate) static MUTATING_FILE_TOOLS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| HashSet::from(["Write", "Edit", "MultiEdit", "NotebookEdit"]));

fn workers_selected_in_routing_cache() -> bool {
    let cached = load_json_object(&crate::env::cache_dir().join("usage-routing.json"));
    let Some(summary) = cached.as_ref().and_then(|c| c.get("summary")) else {
        return false;
    };
    summary
        .get("selected_workers")
        .and_then(Value::as_array)
        .is_some_and(|workers| !workers.is_empty())
}

fn delegation_required() -> bool {
    if !env_truthy("CLAUDEX_SUBAGENT_FIRST", true) {
        return false;
    }
    if env_truthy("CLAUDEX_ALLOW_MAIN_TOOLS", false) {
        return false;
    }
    let state_path = crate::env::cache_dir().join("delegation-state.json");
    if let Some(state) = load_json_object(&state_path)
        && state.get("delegation_required").is_some()
    {
        return state
            .get("delegation_required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    }
    workers_selected_in_routing_cache()
}

fn transcript_marks_subagent(payload: &Map<String, Value>) -> bool {
    ["transcript_path", "agent_transcript_path"]
        .into_iter()
        .filter_map(|key| nonempty_str(payload.get(key)))
        .any(|path| path.replace('\\', "/").contains("/subagents/"))
}

fn is_subagent_session(payload: &Map<String, Value>) -> bool {
    nonempty_str(payload.get("agent_id")).is_some()
        || nonempty_str(payload.get("agent_type")).is_some()
        || transcript_marks_subagent(payload)
}

fn maybe_acquire_file_locks(payload: &Map<String, Value>, tool_input: &Map<String, Value>) -> Option<Value> {
    let paths = tool_file_paths("", tool_input);
    if paths.is_empty() {
        return None;
    }
    acquire_locks(payload, &paths)
}

fn handle_subagent_pre_tool_use(
    payload: &Map<String, Value>,
    tool_name: &str,
    tool_input: &Map<String, Value>,
) -> Value {
    if MUTATING_FILE_TOOLS.contains(tool_name)
        && let Some(denied) = maybe_acquire_file_locks(payload, tool_input)
    {
        return denied;
    }
    if DENIED_MAIN_TOOLS.contains(tool_name) {
        return allow(
            Some("PreToolUse"),
            Some(
                "Claudex SubAgent keeps the full tool set; main-session \
                 Read/Write/Edit/search denials do not apply here.",
            ),
        );
    }
    allow(None, None)
}

fn deny_main_tool(tool_name: &str) -> Value {
    deny(
        "PreToolUse",
        &format!(
            "Claudex main session must not run `{tool_name}` while routed workers are \
             available. Launch Agent/Task with a selected_workers entry and keep \
             file/search work in that SubAgent. Bash remains allowed in main for \
             lightweight orchestration. Set CLAUDEX_ALLOW_MAIN_TOOLS=1 only for an \
             explicit emergency override."
        ),
    )
}

fn handle_pre_tool_use(payload: &Map<String, Value>) -> Value {
    let Some(tool_name) = payload.get("tool_name").and_then(Value::as_str) else {
        return allow(None, None);
    };
    let empty = Map::new();
    let tool_input = payload
        .get("tool_input")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    if is_subagent_session(payload) {
        return handle_subagent_pre_tool_use(payload, tool_name, tool_input);
    }
    if DENIED_MAIN_TOOLS.contains(tool_name) && delegation_required() {
        return deny_main_tool(tool_name);
    }
    allow(None, None)
}

fn handle_post_tool_use(payload: &Map<String, Value>) -> Value {
    let tool_name = payload.get("tool_name").and_then(Value::as_str);
    let agent_id = payload.get("agent_id").and_then(Value::as_str);
    let tool_input = payload.get("tool_input").and_then(Value::as_object);
    if let (Some(tool_name), Some(agent_id), Some(tool_input)) = (tool_name, agent_id, tool_input)
        && MUTATING_FILE_TOOLS.contains(tool_name)
    {
        release_paths(agent_id, &tool_file_paths(tool_name, tool_input));
    }
    allow(None, None)
}

fn handle_subagent_stop(payload: &Map<String, Value>) -> Value {
    if let Some(agent_id) = nonempty_str(payload.get("agent_id")) {
        release_agent_locks(agent_id);
    }
    allow(None, None)
}

/// Dispatch a Claude Code hook event payload.
pub fn handle_event(payload: &Map<String, Value>) -> Value {
    match payload.get("hook_event_name").and_then(Value::as_str) {
        Some("PreToolUse") => handle_pre_tool_use(payload),
        Some("PostToolUse") => handle_post_tool_use(payload),
        Some("SubagentStop") => handle_subagent_stop(payload),
        _ => allow(None, None),
    }
}
