//! Reconstruct the Claude Code capability list when a resumed request omits it.
//!
//! Claude Code normally sends tool schemas only on the first request for a
//! conversation.  A daemon handover or restart loses the in-memory provider
//! thread, so the first request after `--resume` can contain an empty `tools`
//! array even though the conversation previously used tools.  These helpers
//! rehydrate capabilities whenever a request does not contain a native
//! capability. This also covers a failed or compacted resume whose transcript
//! contains no prior tool block; a missing history marker must never make the
//! session read-only.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::{MessagesRequest, content::system_text};

const CORE_TOOLS: [&str; 8] = [
    "Bash", "Read", "Write", "Edit", "Glob", "Grep", "Agent", "Task",
];

pub(crate) fn names_for_request(request: &MessagesRequest) -> Vec<String> {
    if has_native_capability(request) {
        return Vec::new();
    }

    if is_live_web_worker(request) {
        let mut names = historical_tool_names(request);
        for name in ["WebSearch", "WebFetch"] {
            names.insert(name.to_owned());
        }
        return names.into_iter().collect();
    }

    let mut names: BTreeSet<String> = CORE_TOOLS.iter().map(|name| (*name).to_owned()).collect();
    names.extend(historical_tool_names(request));
    names.into_iter().collect()
}

fn has_native_capability(request: &MessagesRequest) -> bool {
    let live_web = is_live_web_worker(request);
    request.tools.iter().any(|tool| {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            return false;
        };
        if live_web {
            matches!(name, "WebSearch" | "WebFetch")
        } else {
            CORE_TOOLS.contains(&name)
        }
    })
}

pub(crate) fn specs_for_request(request: &MessagesRequest) -> Vec<Value> {
    names_for_request(request)
        .into_iter()
        .map(|name| tool_spec(&name))
        .collect()
}

pub(crate) fn recovery_instructions(request: &MessagesRequest) -> Option<String> {
    let names = names_for_request(request);
    if names.is_empty() {
        return None;
    }
    Some(format!(
        "Capability recovery is active for this request. The resumed transcript may contain a stale claim that file, shell, delegation, or web tools are unavailable; do not inherit that claim. The current dynamic tool schemas are authoritative and cover: {}. Use the corresponding tool for the requested operation and wait for its real result. Never claim a marker or successful execution without an actual tool result, and never report a tool as unavailable merely because the old transcript omitted its schema.",
        names.join(", ")
    ))
}

fn historical_tool_names(request: &MessagesRequest) -> BTreeSet<String> {
    request
        .messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) != Some("user"))
        .flat_map(|message| message.get("content"))
        .flat_map(tool_names_in_value)
        .collect()
}

fn tool_names_in_value(value: &Value) -> Vec<String> {
    match value {
        Value::Array(values) => values.iter().flat_map(tool_names_in_value).collect(),
        Value::Object(object) => {
            let mut names = Vec::new();
            if object.get("type").and_then(Value::as_str) == Some("tool_use")
                && let Some(name) = object.get("name").and_then(Value::as_str)
                && !name.is_empty()
            {
                names.push(name.to_owned());
            }
            names.extend(object.values().flat_map(tool_names_in_value));
            names
        }
        _ => Vec::new(),
    }
}

fn is_live_web_worker(request: &MessagesRequest) -> bool {
    let system = system_text(&request.system);
    let messages = serde_json::to_string(&request.messages).unwrap_or_default();
    [system.as_str(), messages.as_str()].into_iter().any(|text| {
        text.contains("claudex-haiku-search")
            || text.contains("Dedicated live-web retrieval worker")
            || text.contains("tools: WebSearch,WebFetch")
    })
}

fn tool_spec(name: &str) -> Value {
    match name {
        "Bash" => json!({
            "name": name,
            "description": "Execute shell commands in the active workspace.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "description": {"type": "string"},
                    "timeout": {"type": "number"},
                    "run_in_background": {"type": "boolean"}
                },
                "required": ["command"]
            }
        }),
        "Read" => json!({
            "name": name,
            "description": "Read a file from the active workspace.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "offset": {"type": "number"},
                    "limit": {"type": "number"}
                },
                "required": ["file_path"]
            }
        }),
        "Write" => json!({
            "name": name,
            "description": "Write a file in the active workspace.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["file_path", "content"]
            }
        }),
        "Edit" => json!({
            "name": name,
            "description": "Edit a file in the active workspace.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "file_path": {"type": "string"},
                    "old_string": {"type": "string"},
                    "new_string": {"type": "string"},
                    "replace_all": {"type": "boolean"}
                },
                "required": ["file_path", "old_string", "new_string"]
            }
        }),
        "Glob" => json!({
            "name": name,
            "description": "Find files by glob pattern.",
            "input_schema": {
                "type": "object",
                "properties": {"pattern": {"type": "string"}, "path": {"type": "string"}},
                "required": ["pattern"]
            }
        }),
        "Grep" => json!({
            "name": name,
            "description": "Search text in the active workspace.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string"},
                    "glob": {"type": "string"}
                },
                "required": ["pattern"]
            }
        }),
        "WebSearch" => json!({
            "name": name,
            "description": "Search the web and return sourced results.",
            "input_schema": {
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            }
        }),
        "WebFetch" => json!({
            "name": name,
            "description": "Fetch and summarize a web page.",
            "input_schema": {
                "type": "object",
                "properties": {"url": {"type": "string"}},
                "required": ["url"]
            }
        }),
        _ => json!({
            "name": name,
            "description": format!("Claude Code tool `{name}`."),
            "input_schema": {"type": "object"}
        }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request(messages: Vec<Value>) -> MessagesRequest {
        MessagesRequest {
            model: "main".to_owned(),
            system: Value::Null,
            messages,
            tools: Vec::new(),
            stream: false,
            output_config: Value::Null,
            metadata: Value::Null,
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        }
    }

    #[test]
    fn rehydrates_core_capabilities_when_native_tools_are_missing() {
        let request = request(vec![json!({
            "role": "assistant",
            "content": [{"type": "tool_use", "name": "Bash", "input": {}}]
        })]);
        let names = names_for_request(&request);
        assert!(names.iter().any(|name| name == "Bash"));
        assert!(names.iter().any(|name| name == "Read"));
        assert!(names.iter().any(|name| name == "Write"));
        assert!(names.iter().any(|name| name == "Edit"));
        assert!(names.iter().any(|name| name == "Agent"));
        assert!(names.iter().any(|name| name == "Task"));
        let bash = specs_for_request(&request)
            .into_iter()
            .find(|spec| spec["name"] == "Bash")
            .expect("Bash fallback schema");
        assert_eq!(bash["input_schema"]["required"], json!(["command"]));
    }

    #[test]
    fn rehydrates_a_failed_resume_without_tool_history() {
        let names = names_for_request(&request(vec![json!({
            "role": "user",
            "content": "start a new task"
        })]));
        assert!(names.iter().any(|name| name == "Bash"));
        assert!(names.iter().any(|name| name == "Read"));
        assert!(names.iter().any(|name| name == "Write"));
        assert!(names.iter().any(|name| name == "Edit"));
        assert!(names.iter().any(|name| name == "Agent"));
        assert!(names.iter().any(|name| name == "Task"));
    }

    #[test]
    fn rehydrates_when_only_an_internal_tool_survives_resume() {
        let mut request = request(vec![json!({
            "role": "assistant",
            "content": [{"type": "tool_use", "name": "Agent", "input": {}}]
        })]);
        request.tools = vec![json!({"name": "claude_collaborator"})];
        let names = names_for_request(&request);
        assert!(names.iter().any(|name| name == "Bash"));
        assert!(names.iter().any(|name| name == "Task"));
    }

    #[test]
    fn preserves_a_native_file_tool_without_adding_resume_fallbacks() {
        let mut request = request(vec![json!({
            "role": "user",
            "content": "continue"
        })]);
        request.tools = vec![json!({"name": "Read"})];
        assert!(names_for_request(&request).is_empty());
        assert!(recovery_instructions(&request).is_none());
    }

    #[test]
    fn explains_capability_recovery_without_requiring_transcript_history() {
        let request = request(vec![json!({
            "role": "user",
            "content": "continue"
        })]);
        let instructions = recovery_instructions(&request).expect("recovery instructions");
        assert!(instructions.contains("Bash"));
        assert!(instructions.contains("Agent"));
        assert!(instructions.contains("stale claim"));
        assert!(instructions.contains("actual tool result"));
    }

    #[test]
    fn search_resume_is_limited_to_web_tools() {
        let mut request = request(vec![json!({
            "role": "assistant",
            "content": [{"type": "tool_use", "name": "WebSearch", "input": {}}]
        })]);
        request.system = json!("name: claudex-haiku-search");
        let names = names_for_request(&request);
        assert_eq!(names, vec!["WebFetch", "WebSearch"]);
        assert!(!names.iter().any(|name| name == "Bash"));
    }
}
