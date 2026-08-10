use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Result, bail};
use serde_json::Value;

use super::super::{
    Session,
    content::{ToolResult, matching_transcript_len},
};

pub(super) fn touch_session(session: &Session) {
    *session
        .last_activity
        .lock()
        .expect("session clock poisoned") = std::time::Instant::now();
}

pub(super) fn owns_tool_result(
    pending: &HashMap<String, Value>,
    consumed: &HashSet<String>,
    tool_use_id: &str,
) -> bool {
    pending.contains_key(tool_use_id) || consumed.contains(tool_use_id)
}

pub(super) fn is_better_length(best: Option<usize>, candidate: usize) -> bool {
    match best {
        Some(best) => candidate > best,
        None => true,
    }
}

pub(super) fn validate_tool_result_ownership(
    pending: &HashMap<String, Value>,
    consumed: &HashSet<String>,
    tool_results: &[ToolResult],
) -> Result<()> {
    if tool_results
        .iter()
        .all(|result| owns_tool_result(pending, consumed, &result.tool_use_id))
    {
        return Ok(());
    }
    bail!("Claude tool results were already consumed by another request");
}

pub(super) fn should_preempt_for_context_limit(
    input_tokens: u64,
    limit: Option<u64>,
    has_tool_results: bool,
) -> bool {
    limit.is_some_and(|limit| !has_tool_results && input_tokens >= limit)
}

pub(super) fn is_idempotent_task_lifecycle_error(content_items: &[Value]) -> bool {
    // Claude Code reports a consumed TaskStop target as an error even though
    // the requested stop is already satisfied. TaskOutput `No task found` that
    // still lists `Running background agents` is a miss, not completed output.
    content_items.iter().any(|item| {
        let Some(text) = item.get("text").and_then(Value::as_str) else {
            return false;
        };
        is_idempotent_task_lifecycle_text(text)
    })
}

fn is_idempotent_task_lifecycle_text(text: &str) -> bool {
    let normalized = text
        .replace("<tool_use_error>", " ")
        .replace("</tool_use_error>", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    // Reject shell transcripts that merely quote the phrase.
    if normalized.starts_with("shell output:") {
        return false;
    }
    if normalized.contains("running background agents:") {
        return false;
    }
    normalized.starts_with("error: no task found with id:")
        || normalized.starts_with("no task found with id:")
        || (normalized.starts_with("error: task ")
            && normalized.contains(" is not running (status: completed)"))
        || (normalized.starts_with("task ")
            && normalized.contains(" is not running (status: completed)"))
}

pub(super) async fn candidate_length(
    session: &Arc<Session>,
    signature: &Arc<str>,
    messages: &[Value],
) -> Option<usize> {
    if !signatures_compatible(session.signature.as_ref(), signature.as_ref()) {
        return None;
    }
    matching_transcript_len(session, messages).await
}

/// Exact signature first. Claude Code often rewrites `system` or tool schema
/// bodies between HTTP turns; those must not cold-start a new ACP `session/new`.
/// Identity is transport + cwd + tool *names* + routing flags, not prompt text.
fn signatures_compatible(stored: &str, requested: &str) -> bool {
    if stored == requested {
        return true;
    }
    match (
        stable_signature_identity(stored),
        stable_signature_identity(requested),
    ) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct StableSignatureIdentity {
    transport_identity: Value,
    working_directory: Value,
    tool_names: Vec<String>,
    disabled_subagent_models: Value,
    advisor_model: Value,
    collaborator_model: Value,
    spawn_limit: Value,
    user_metadata: Value,
}

fn stable_signature_identity(signature: &str) -> Option<StableSignatureIdentity> {
    let parsed: Value = serde_json::from_str(signature).ok()?;
    let object = parsed.as_object()?;
    let tools = object.get("tools")?.as_array()?;
    let mut tool_names: Vec<String> = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect();
    if !tools.is_empty() && tool_names.len() != tools.len() {
        return None;
    }
    let transport = object
        .get("transport_identity")
        .cloned()
        .unwrap_or(Value::Null);
    if !usable_transport_identity(&transport) {
        return None;
    }
    tool_names.sort_unstable();
    Some(StableSignatureIdentity {
        transport_identity: transport,
        working_directory: object
            .get("working_directory")
            .cloned()
            .unwrap_or(Value::Null),
        tool_names,
        disabled_subagent_models: object
            .get("disabled_subagent_models")
            .cloned()
            .unwrap_or(Value::Null),
        advisor_model: object.get("advisor_model").cloned().unwrap_or(Value::Null),
        collaborator_model: object
            .get("collaborator_model")
            .cloned()
            .unwrap_or(Value::Null),
        spawn_limit: object
            .get("subagent_spawn_limit_reached")
            .cloned()
            .unwrap_or(Value::Null),
        user_metadata: object.get("metadata").cloned().unwrap_or(Value::Null),
    })
}

fn usable_transport_identity(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    ["session_id", "agent_id"].into_iter().any(|key| {
        object
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use serde_json::json;

    fn signature(system: &str, tools: Value, transport: Value) -> String {
        serde_json::to_string(&json!({
            "system": system,
            "tools": tools,
            "metadata": "client",
            "transport_identity": transport,
            "subagent_spawn_limit_reached": null,
            "working_directory": "/tmp/proj",
            "disabled_subagent_models": [],
            "advisor_model": null,
            "collaborator_model": null
        }))
        .expect("signature json")
    }

    #[test]
    fn exact_strings_are_compatible_without_json() {
        assert!(signatures_compatible("plain", "plain"));
        assert!(!signatures_compatible("plain", "other"));
    }

    #[test]
    fn system_text_drift_stays_compatible() {
        let tools = json!([{"name": "Bash"}]);
        let transport = json!({"session_id": "sess-1", "agent_id": null});
        assert!(signatures_compatible(
            &signature("old", tools.clone(), transport.clone()),
            &signature("new", tools, transport),
        ));
    }

    #[test]
    fn main_session_id_without_agent_id_is_usable() {
        let tools = json!([]);
        let left = signature("s", tools.clone(), json!({"session_id": "sess-1"}));
        let right = signature("s2", tools, json!({"session_id": "sess-1"}));
        assert!(signatures_compatible(&left, &right));
    }

    #[test]
    fn agent_id_without_session_id_is_usable() {
        let tools = json!([{"name": "Read"}]);
        let left = signature("s", tools.clone(), json!({"agent_id": "agent-a"}));
        let right = signature("s2", tools, json!({"agent_id": "agent-a"}));
        assert!(signatures_compatible(&left, &right));
    }

    #[test]
    fn empty_transport_ids_are_not_a_stable_identity() {
        let tools = json!([]);
        let empty = signature(
            "s",
            tools.clone(),
            json!({"session_id": "", "agent_id": ""}),
        );
        let also_empty = signature("s2", tools, json!({"session_id": null, "agent_id": null}));
        assert!(!signatures_compatible(&empty, &also_empty));
        assert!(!usable_transport_identity(&json!(null)));
    }

    #[test]
    fn unnamed_tools_disable_stable_identity() {
        let transport = json!({"session_id": "sess-1"});
        let named = signature("s", json!([{"name": "Bash"}]), transport.clone());
        let unnamed = signature("s", json!([{"description": "no name"}]), transport);
        assert!(!signatures_compatible(&named, &unnamed));
    }
}
