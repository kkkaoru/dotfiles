use std::collections::HashMap;

use serde_json::{Value, json};

use super::super::{BRIDGE_INSTRUCTIONS, MessagesRequest, content::system_text};

pub(super) fn tool_configuration(
    request: &MessagesRequest,
    advisor_model: Option<&str>,
    collaborator_model: Option<&str>,
) -> (Vec<Value>, HashMap<String, String>, HashMap<String, String>) {
    let (mut tools, external_names) = external_tools(&request.tools);
    let mut internal = HashMap::new();
    if let Some(model) = advisor_model {
        internal.insert("advisor".to_owned(), model.to_owned());
        tools.push(internal_advisor_tool());
    }
    let has_collaborator = request
        .tools
        .iter()
        .any(|tool| tool["name"] == "claude_collaborator");
    if let Some(model) = collaborator_model.filter(|_| !has_collaborator) {
        internal.insert("claude_collaborator".to_owned(), model.to_owned());
        tools.push(internal_collaborator_tool());
    }
    (tools, external_names, internal)
}

fn external_tools(tools: &[Value]) -> (Vec<Value>, HashMap<String, String>) {
    let mut specs = Vec::new();
    let mut names = HashMap::new();
    for (index, tool) in tools.iter().enumerate() {
        let Some(original_name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        let codex_name = codex_tool_name(original_name, index);
        if let Some(spec) = dynamic_tool(tool, &codex_name) {
            names.insert(codex_name, original_name.to_owned());
            specs.push(spec);
        }
    }
    (specs, names)
}

pub(super) fn thread_start_params(
    request: &MessagesRequest,
    model: &str,
    dynamic_tools: Vec<Value>,
) -> Value {
    let system = system_text(&request.system);
    let developer_instructions = super::super::team_protocol::guidance(&request.tools).map_or_else(
        || BRIDGE_INSTRUCTIONS.to_owned(),
        |guidance| format!("{BRIDGE_INSTRUCTIONS}\n\n{guidance}"),
    );
    let base_instructions = if system.is_empty() {
        developer_instructions.clone()
    } else {
        format!("{system}\n\n{developer_instructions}")
    };
    json!({
        "model": model,
        "cwd": isolated_runtime_cwd(),
        "baseInstructions": base_instructions,
        "developerInstructions": developer_instructions,
        "dynamicTools": dynamic_tools,
        "environments": [],
        "ephemeral": true,
        "approvalPolicy": "never",
        "sandbox": "read-only",
        "personality": "none",
        "config": {
            "web_search": "disabled",
            "features": {
                "apps": false, "multi_agent": false, "shell_tool": false,
                "tool_search": false, "unified_exec": false, "web_search": false
            }
        }
    })
}

pub(in crate::anthropic) fn dynamic_tool(tool: &Value, codex_name: &str) -> Option<Value> {
    let original_name = tool.get("name")?.as_str()?;
    Some(json!({
        "type": "function",
        "name": codex_name,
        "description": format!(
            "Claude Code tool `{original_name}`. {}",
            tool.get("description").and_then(Value::as_str).unwrap_or("")
        ),
        "inputSchema": super::super::agent_effort::tool_schema(original_name,
            tool.get("input_schema").cloned()
                .unwrap_or_else(|| json!({"type":"object"})))
    }))
}

pub(in crate::anthropic) fn codex_tool_name(original_name: &str, index: usize) -> String {
    let sanitized = original_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let suffix = format!("_{index}");
    let maximum_name_bytes = 128usize.saturating_sub(3 + suffix.len());
    let stem = &sanitized[..sanitized.len().min(maximum_name_bytes)];
    format!("cc_{stem}{suffix}")
}

fn isolated_runtime_cwd() -> String {
    let home = match std::env::var_os("HOME") {
        Some(home) => std::path::PathBuf::from(home),
        None => std::path::PathBuf::from("/tmp"),
    };
    home.join(".cache/claudex/codex-home")
        .to_string_lossy()
        .into_owned()
}

pub(in crate::anthropic) fn internal_advisor_tool() -> Value {
    json!({
        "type":"function",
        "name":"advisor",
        "description":"Ask the advisor model configured by Claude Code to independently review the entire conversation and return high-value guidance. It takes no parameters.",
        "inputSchema":{"type":"object","properties":{},"additionalProperties":false}
    })
}

pub(in crate::anthropic) fn internal_collaborator_tool() -> Value {
    json!({
        "type":"function",
        "name":"claude_collaborator",
        "description":"Delegate an independent task to the collaborator model configured by Claude Code through the user's Claude subscription. Multiple calls may be issued in parallel.",
        "inputSchema":{
            "type":"object",
            "properties":{"task":{"type":"string","description":"The task for the Claude collaborator."}},
            "required":["task"],
            "additionalProperties":false
        }
    })
}
