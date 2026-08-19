//! Enforce claudex main-session delegation and exclusive file locks.
//!
//! Claude Code hooks feed one JSON event on stdin. When `CLAUDEX_ACTIVE=1`:
//!
//! * PreToolUse in the **main** session denies Write/Edit/MultiEdit/NotebookEdit
//!   while routing says delegation is required. Atomic
//!   Read/Grep/Glob/LS/WebSearch/WebFetch may stay in main. SubAgents keep the
//!   full tool set.
//! * SubAgent identity is detected via `agent_id` / `agentId` / `agent_type` /
//!   `agentType` / subagent transcript path so main-session denials never apply
//!   to workers.
//! * PreToolUse Write/Edit acquires a per-path lock for the calling `agent_id`
//!   or `agentId` so parallel SubAgents cannot mutate the same file at once.
//! * PostToolUse, SubagentStop, and SessionEnd release those locks. Release
//!   accepts `agentId` as well as `agent_id`.
//! * Same-session leftover leases (sequential same-slot relaunch) are stolen
//!   after 90 seconds only when the holder is no longer live. A live sibling is
//!   never stolen just because 90 seconds passed or the TUI shows 1 agent.
//!   Cross-session stale locks still expire after 5 minutes for holders that
//!   are no longer live.
//! * Isolated SubAgent worktrees: a mutating tool whose target is outside
//!   `cwd` when `cwd` contains `/.claude/worktrees/` is denied with the
//!   worktree path. Claude Code otherwise reports only `Error writing file`.
//! * Lock-store failures fail open. Real conflicts name the holder, never
//!   "another agent". A group-readable lock directory is repaired to 0700.

mod env;
mod locks;
mod policy;
mod state;
mod worktree;

pub use policy::{PolicyContext, handle_event, handle_event_with_context};

use serde_json::{Map, Value};
use std::io::{self, Read, Write};

/// Run the hook: read stdin JSON, write decision JSON to stdout.
pub fn run() -> io::Result<i32> {
    if !env::env_truthy("CLAUDEX_ACTIVE", false) || env::is_child_runtime() {
        writeln!(io::stdout(), "{{}}")?;
        return Ok(0);
    }
    let mut raw = String::new();
    io::stdin().read_to_string(&mut raw)?;
    if raw.trim().is_empty() {
        writeln!(io::stdout(), "{{}}")?;
        return Ok(0);
    }
    let Ok(payload) = serde_json::from_str::<Value>(&raw) else {
        writeln!(io::stdout(), "{{}}")?;
        return Ok(0);
    };
    let Some(obj) = payload.as_object() else {
        writeln!(io::stdout(), "{{}}")?;
        return Ok(0);
    };
    let result = handle_event(obj);
    writeln!(
        io::stdout(),
        "{}",
        serde_json::to_string(&result).unwrap_or_else(|_| "{}".into())
    )?;
    Ok(0)
}

pub(crate) fn deny(event_name: &str, reason: &str) -> Value {
    Value::Object(Map::from_iter([
        ("decision".into(), Value::String("block".into())),
        ("reason".into(), Value::String(reason.into())),
        (
            "hookSpecificOutput".into(),
            Value::Object(Map::from_iter([
                ("hookEventName".into(), Value::String(event_name.into())),
                ("permissionDecision".into(), Value::String("deny".into())),
                (
                    "permissionDecisionReason".into(),
                    Value::String(reason.into()),
                ),
            ])),
        ),
    ]))
}

pub(crate) fn allow(event_name: Option<&str>, reason: Option<&str>) -> Value {
    match (event_name, reason) {
        (Some(event), Some(reason)) => Value::Object(Map::from_iter([(
            "hookSpecificOutput".into(),
            Value::Object(Map::from_iter([
                ("hookEventName".into(), Value::String(event.into())),
                ("permissionDecision".into(), Value::String("allow".into())),
                (
                    "permissionDecisionReason".into(),
                    Value::String(reason.into()),
                ),
            ])),
        )])),
        _ => Value::Object(Map::new()),
    }
}
