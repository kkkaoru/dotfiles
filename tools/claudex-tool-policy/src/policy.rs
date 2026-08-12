use crate::allow;
use crate::deny;
use crate::env::nonempty_str;
use crate::locks::{acquire_locks, release_agent_locks, release_paths, tool_file_paths};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// All mutable runtime inputs used by policy evaluation. Tests can construct
/// this directly without changing process-global environment variables.
#[derive(Clone, Debug)]
pub struct PolicyContext {
    cache_dir: PathBuf,
    home_dir: PathBuf,
    now_seconds: f64,
    subagent_first: bool,
    allow_main_tools: bool,
}

impl PolicyContext {
    #[must_use]
    pub fn new(
        cache_dir: PathBuf,
        home_dir: PathBuf,
        now_seconds: f64,
        subagent_first: bool,
        allow_main_tools: bool,
    ) -> Self {
        Self {
            cache_dir,
            home_dir,
            now_seconds,
            subagent_first,
            allow_main_tools,
        }
    }

    pub(crate) fn from_environment() -> Self {
        Self::new(
            crate::env::cache_dir(),
            crate::env::home_dir(),
            crate::env::now_seconds(),
            crate::env::env_truthy("CLAUDEX_SUBAGENT_FIRST", true),
            crate::env::env_truthy("CLAUDEX_ALLOW_MAIN_TOOLS", false),
        )
    }

    pub(crate) fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub(crate) fn home_dir(&self) -> &Path {
        &self.home_dir
    }

    pub(crate) fn now_seconds(&self) -> f64 {
        self.now_seconds
    }

    pub(crate) fn subagent_first(&self) -> bool {
        self.subagent_first
    }

    pub(crate) fn allow_main_tools(&self) -> bool {
        self.allow_main_tools
    }
}

/// Atomic lookups may stay in the main session even when delegation is required.
pub(crate) static ATOMIC_LOOKUP_TOOLS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| HashSet::from(["Read", "Grep", "Glob", "LS", "WebSearch", "WebFetch"]));

pub(crate) static MUTATING_FILE_TOOLS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| HashSet::from(["Write", "Edit", "MultiEdit", "NotebookEdit"]));

fn is_main_session_policy_tool(tool_name: &str) -> bool {
    ATOMIC_LOOKUP_TOOLS.contains(tool_name) || MUTATING_FILE_TOOLS.contains(tool_name)
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

fn maybe_acquire_file_locks(
    payload: &Map<String, Value>,
    tool_input: &Map<String, Value>,
    context: &PolicyContext,
) -> Option<Value> {
    let paths = tool_file_paths("", tool_input);
    if paths.is_empty() {
        return None;
    }
    acquire_locks(payload, &paths, context)
}

fn handle_subagent_pre_tool_use(
    payload: &Map<String, Value>,
    tool_name: &str,
    tool_input: &Map<String, Value>,
    context: &PolicyContext,
) -> Value {
    if MUTATING_FILE_TOOLS.contains(tool_name)
        && let Some(denied) = maybe_acquire_file_locks(payload, tool_input, context)
    {
        return denied;
    }
    if is_main_session_policy_tool(tool_name) {
        return allow(
            Some("PreToolUse"),
            Some(
                "Claudex SubAgent keeps the full tool set; main-session \
                 Write/Edit/MultiEdit/NotebookEdit denials do not apply here.",
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
             Write/Edit/MultiEdit/NotebookEdit in that SubAgent. Atomic \
             Read/Grep/Glob/LS/WebSearch/WebFetch may stay in main. Bash remains \
             allowed in main for lightweight orchestration. Set \
             CLAUDEX_ALLOW_MAIN_TOOLS=1 only for an explicit emergency override."
        ),
    )
}

fn handle_pre_tool_use(payload: &Map<String, Value>, context: &PolicyContext) -> Value {
    let Some(tool_name) = payload.get("tool_name").and_then(Value::as_str) else {
        return allow(None, None);
    };
    let empty = Map::new();
    let tool_input = payload
        .get("tool_input")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    if is_subagent_session(payload) {
        return handle_subagent_pre_tool_use(payload, tool_name, tool_input, context);
    }
    if MUTATING_FILE_TOOLS.contains(tool_name)
        && crate::state::delegation_required(payload, context)
    {
        return deny_main_tool(tool_name);
    }
    allow(None, None)
}

fn handle_post_tool_use(payload: &Map<String, Value>, context: &PolicyContext) -> Value {
    let tool_name = payload.get("tool_name").and_then(Value::as_str);
    let tool_input = payload.get("tool_input").and_then(Value::as_object);
    if let (Some(tool_name), Some(tool_input)) = (tool_name, tool_input)
        && MUTATING_FILE_TOOLS.contains(tool_name)
        && nonempty_str(payload.get("agent_id")).is_some()
    {
        release_paths(payload, &tool_file_paths(tool_name, tool_input), context);
    }
    allow(None, None)
}

fn handle_subagent_stop(payload: &Map<String, Value>, context: &PolicyContext) -> Value {
    // Claude Code sets this while replaying a Stop/SubagentStop continuation.
    // Never perform stateful hook work on the recursive invocation; returning
    // an unconditional success is the documented recursion fence.
    if payload
        .get("stop_hook_active")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return allow(None, None);
    }
    if nonempty_str(payload.get("agent_id")).is_some() {
        release_agent_locks(payload, context);
    }
    allow(None, None)
}

/// Dispatch a Claude Code hook event payload.
pub fn handle_event(payload: &Map<String, Value>) -> Value {
    handle_event_with_context(payload, &PolicyContext::from_environment())
}

/// Dispatch using explicit cache, clock, and override inputs.
pub fn handle_event_with_context(payload: &Map<String, Value>, context: &PolicyContext) -> Value {
    match payload.get("hook_event_name").and_then(Value::as_str) {
        Some("PreToolUse") => handle_pre_tool_use(payload, context),
        Some("PostToolUse") => handle_post_tool_use(payload, context),
        Some("SubagentStop") => handle_subagent_stop(payload, context),
        _ => allow(None, None),
    }
}
