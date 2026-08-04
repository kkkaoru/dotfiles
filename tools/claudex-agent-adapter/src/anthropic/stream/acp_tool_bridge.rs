//! ACP provider tools → Claude Code `tool_use` only for client-executed tools.
//!
//! Provider-native tools (bash/read/edit/…) stay as WIP progress. Bridging them would
//! double-execute under Claude Code. Launch tools (`Agent`/`Task`) and Grok-native
//! `spawn_subagent` (mapped onto Agent when Claude Code supplied Agent) become
//! Anthropic `tool_use`.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::ToolCall;

/// Pending-tool request id marker: ACP cannot accept Codex-style tool results on the
/// app-server channel; follow-up turns continue via transcript + `turn/start`.
pub(super) const ACP_BRIDGE_MARKER: &str = "acpBridge";

const SPAWN_SUBAGENT: &str = "spawn_subagent";
const GROK_HIGH_PROFILE: &str = "grok-native-high-plugin-v3:claudex-high";

pub(super) fn is_client_executed_bridge_tool(original_name: &str) -> bool {
    matches!(original_name, "Agent" | "Task")
}

pub(in crate::anthropic) fn is_acp_bridge_request_id(id: &Value) -> bool {
    id.get(ACP_BRIDGE_MARKER).and_then(Value::as_bool) == Some(true)
}

pub(super) fn acp_bridge_request_id(call_id: &str) -> Value {
    json!({ ACP_BRIDGE_MARKER: true, "callId": call_id })
}

fn bridgeable_status(status: Option<&str>) -> bool {
    match status.unwrap_or("pending") {
        "pending" | "in_progress" | "started" => true,
        "completed" | "failed" | "cancelled" => false,
        _ => true,
    }
}

fn requested_original_name<'a>(
    names: &'a HashMap<String, String>,
    provider_name: &str,
) -> Option<&'a str> {
    names.get(provider_name).map(String::as_str).or_else(|| {
        names
            .values()
            .find(|name| name.as_str() == provider_name)
            .map(String::as_str)
    })
}

fn has_agent_tool(names: &HashMap<String, String>) -> bool {
    names.values().any(|name| name == "Agent")
        || names.contains_key("Agent")
        || names.keys().any(|key| key.ends_with("Agent") || key.contains("Agent"))
}

fn looks_like_launch_tool(candidate: &str) -> bool {
    let lower = candidate.to_ascii_lowercase();
    lower == "agent"
        || lower == "task"
        || lower == SPAWN_SUBAGENT
        || lower.ends_with("__agent")
        || lower.ends_with("__task")
        || lower.contains("spawn_subagent")
}

fn map_launch_name(candidate: &str, names: &HashMap<String, String>) -> Option<String> {
    if let Some(original) = requested_original_name(names, candidate)
        && is_client_executed_bridge_tool(original)
    {
        return Some(original.to_owned());
    }
    if looks_like_launch_tool(candidate) && has_agent_tool(names) {
        if candidate.eq_ignore_ascii_case("task")
            || candidate.to_ascii_lowercase().ends_with("__task")
        {
            if names.values().any(|name| name == "Task") {
                return Some("Task".to_owned());
            }
        }
        return Some("Agent".to_owned());
    }
    None
}

fn normalize_launch_arguments(provider_name: &str, arguments: &Value) -> Value {
    let mut mapped = match arguments {
        Value::Object(map) => Value::Object(map.clone()),
        other => json!({"value": other}),
    };
    let Some(object) = mapped.as_object_mut() else {
        return mapped;
    };
    if provider_name.eq_ignore_ascii_case(SPAWN_SUBAGENT)
        || provider_name.to_ascii_lowercase().contains("spawn_subagent")
    {
        if let Some(subagent_type) = object.get("subagent_type").and_then(Value::as_str) {
            if subagent_type == GROK_HIGH_PROFILE || subagent_type.ends_with(":claudex-high") {
                object.insert("subagent_type".to_owned(), json!("claudex-grok"));
            }
        }
        object
            .entry("run_in_background".to_owned())
            .or_insert(json!(true));
    }
    mapped
}

/// If this providerTool event is a request-supplied Agent/Task launch (or Grok
/// spawn_subagent when Agent is available), return a ToolCall for Claude Code.
pub(super) fn bridge_provider_tool_call(
    external_tool_names: &HashMap<String, String>,
    event: &Value,
) -> Option<ToolCall> {
    let params = event.get("params")?;
    if !bridgeable_status(params.get("status").and_then(Value::as_str)) {
        return None;
    }
    let call_id = params.get("callId").and_then(Value::as_str)?;
    let tool = params.get("tool").and_then(Value::as_str).unwrap_or("");
    let title = params.get("title").and_then(Value::as_str).unwrap_or("");
    let (provider_label, name) = [tool, title]
        .into_iter()
        .filter(|candidate| !candidate.is_empty())
        .find_map(|candidate| {
            map_launch_name(candidate, external_tool_names).map(|name| (candidate, name))
        })?;
    let raw_args = params.get("arguments").unwrap_or(&Value::Null);
    let arguments = normalize_launch_arguments(provider_label, raw_args);
    Some(ToolCall {
        call_id: call_id.to_owned(),
        name,
        arguments,
        request_id: acp_bridge_request_id(call_id),
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn names() -> HashMap<String, String> {
        HashMap::from([
            ("cc_Agent_0".to_owned(), "Agent".to_owned()),
            ("cc_Bash_1".to_owned(), "Bash".to_owned()),
            ("cc_Task_2".to_owned(), "Task".to_owned()),
        ])
    }

    #[test]
    fn bridges_only_agent_and_task_when_request_supplied() {
        assert!(is_client_executed_bridge_tool("Agent"));
        assert!(is_client_executed_bridge_tool("Task"));
        assert!(!is_client_executed_bridge_tool("Bash"));
        let map = names();
        let agent = json!({
            "params":{
                "callId":"c1",
                "tool":"Agent",
                "status":"pending",
                "arguments":{"prompt":"do work"}
            }
        });
        let bridged = bridge_provider_tool_call(&map, &agent).expect("Agent bridges");
        assert_eq!(bridged.call_id, "c1");
        assert_eq!(bridged.name, "Agent");
        assert!(is_acp_bridge_request_id(&bridged.request_id));
        assert_eq!(bridged.arguments["prompt"], "do work");
        let task = json!({
            "params":{
                "callId":"c2",
                "tool":"cc_Task_2",
                "status":"in_progress",
                "arguments":{"prompt":"explore"}
            }
        });
        assert_eq!(
            bridge_provider_tool_call(&map, &task)
                .expect("Task bridges")
                .name,
            "Task"
        );
    }

    #[test]
    fn bridges_spawn_subagent_onto_agent_when_claude_supplied_agent() {
        let map = names();
        let spawn = json!({
            "params":{
                "callId":"s1",
                "tool":"spawn_subagent",
                "status":"pending",
                "arguments":{
                    "description":"smoke",
                    "prompt":"CHILD_OK",
                    "subagent_type":"grok-native-high-plugin-v3:claudex-high"
                }
            }
        });
        let bridged = bridge_provider_tool_call(&map, &spawn).expect("spawn bridges");
        assert_eq!(bridged.name, "Agent");
        assert_eq!(bridged.arguments["subagent_type"], "claudex-grok");
        assert_eq!(bridged.arguments["run_in_background"], true);
        assert_eq!(bridged.arguments["description"], "smoke");
    }

    #[test]
    fn never_bridges_native_tools_or_completed_calls() {
        let map = names();
        let bash = json!({
            "params":{
                "callId":"b1",
                "tool":"Bash",
                "status":"pending",
                "arguments":{"command":"ls"}
            }
        });
        assert!(bridge_provider_tool_call(&map, &bash).is_none());
        let completed = json!({
            "params":{
                "callId":"c3",
                "tool":"Agent",
                "status":"completed",
                "arguments":{"prompt":"late"}
            }
        });
        assert!(bridge_provider_tool_call(&map, &completed).is_none());
        assert!(bridge_provider_tool_call(&HashMap::new(), &json!({
            "params":{"callId":"c4","tool":"Agent","status":"pending","arguments":{}}
        })).is_none());
    }

    #[test]
    fn matches_title_when_tool_label_is_generic() {
        let map = names();
        let event = json!({
            "params":{
                "callId":"t1",
                "tool":"Tool",
                "title":"Agent",
                "status":"pending",
                "arguments":{"prompt":"via title"}
            }
        });
        let bridged = bridge_provider_tool_call(&map, &event).expect("title Agent bridges");
        assert_eq!(bridged.name, "Agent");
    }
}
