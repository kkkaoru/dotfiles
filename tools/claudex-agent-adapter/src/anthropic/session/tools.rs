use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Value, json};

use super::super::{BRIDGE_INSTRUCTIONS, MessagesRequest, content::system_text};
use crate::anthropic::subscription_request::cwd_from_system;

const ORCHESTRATOR_INSTRUCTIONS: &str = "Claudex main-session orchestration mode is active. Coordinate, decompose, delegate, monitor, resolve conflicts, synthesize worker results, and deliver the final response. For every substantive investigation, implementation, review, test, or validation, call a routed Agent/Task worker instead of doing the work in main. Direct filesystem, shell, search, edit, web, and external-work tools are intentionally available only inside worker sessions. This remains mandatory after long execution, compaction, resume, context reconstruction, and worker failure.";

pub(in crate::anthropic) fn tool_configuration(
    request: &MessagesRequest,
    advisor_model: Option<&str>,
    collaborator_model: Option<&str>,
) -> (Vec<Value>, HashMap<String, String>, HashMap<String, String>) {
    let orchestrator_only = !super::super::agent_effort::is_subagent_request(request);
    let selected_agents = selected_agents(request);
    let (mut tools, external_names) =
        external_tools(&request.tools, orchestrator_only, &selected_agents);
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

fn external_tools(
    tools: &[Value],
    orchestrator_only: bool,
    selected_agents: &[String],
) -> (Vec<Value>, HashMap<String, String>) {
    let mut specs = Vec::new();
    let mut names = HashMap::new();
    for (index, tool) in tools.iter().enumerate() {
        let Some(original_name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        if orchestrator_only && !is_orchestration_tool(original_name) {
            continue;
        }
        let mut routed_tool = tool.clone();
        if orchestrator_only && super::super::agent_batch::supports(original_name) {
            constrain_agent_types(&mut routed_tool, selected_agents);
        }
        let codex_name = codex_tool_name(original_name, index);
        if let Some(spec) = dynamic_tool(&routed_tool, &codex_name) {
            names.insert(codex_name, original_name.to_owned());
            specs.push(spec);
        }
        if super::super::agent_batch::supports(original_name) {
            let batch_name = codex_tool_name(&format!("{original_name}_batch"), index);
            if let Some(spec) = super::super::agent_batch::dynamic_tool(&routed_tool, &batch_name) {
                names.insert(
                    batch_name,
                    super::super::agent_batch::mapped_name(original_name),
                );
                specs.push(spec);
            }
        }
    }
    (specs, names)
}

fn is_orchestration_tool(name: &str) -> bool {
    matches!(
        name,
        "Agent"
            | "Task"
            | "SendMessage"
            | "AskUserQuestion"
            | "Skill"
            | "EnterPlanMode"
            | "ExitPlanMode"
            | "TodoWrite"
    ) || name.starts_with("Task")
        || name.starts_with("Team")
}

fn constrain_agent_types(tool: &mut Value, selected_agents: &[String]) {
    if selected_agents.is_empty() {
        return;
    }
    let Some(property) = tool
        .pointer_mut("/input_schema/properties/subagent_type")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    property.insert("enum".to_owned(), json!(selected_agents));
    property.insert(
        "description".to_owned(),
        Value::String(format!(
            "Required routed Claudex worker. Choose exactly one of: {}.",
            selected_agents.join(", ")
        )),
    );
}

fn selected_agents(request: &MessagesRequest) -> Vec<String> {
    routing_texts(&request.system)
        .chain(request.messages.iter().flat_map(routing_texts))
        .find_map(routing_summary)
        .and_then(|summary| summary.get("selected_agents").cloned())
        .and_then(|agents| serde_json::from_value(agents).ok())
        .unwrap_or_default()
}

fn routing_summary(text: &str) -> Option<Value> {
    let start = text.find("{\"providers\":")?;
    Value::deserialize(&mut serde_json::Deserializer::from_str(&text[start..])).ok()
}

fn routing_texts(value: &Value) -> Box<dyn Iterator<Item = &str> + '_> {
    match value {
        Value::String(text) => Box::new(std::iter::once(text.as_str())),
        Value::Array(items) => Box::new(items.iter().flat_map(routing_texts)),
        Value::Object(object) => Box::new(object.values().flat_map(routing_texts)),
        _ => Box::new(std::iter::empty()),
    }
}

pub(in crate::anthropic) fn thread_start_params(
    request: &MessagesRequest,
    model: &str,
    dynamic_tools: Vec<Value>,
) -> Value {
    let system = system_text(&request.system);
    let cwd = request
        .working_directory
        .clone()
        .or_else(|| cwd_from_system(&system))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(isolated_runtime_cwd);
    let mut developer_instructions = super::super::team_protocol::guidance(&request.tools).map_or_else(
        || BRIDGE_INSTRUCTIONS.to_owned(),
        |guidance| format!("{BRIDGE_INSTRUCTIONS}\n\n{guidance}"),
    );
    if !super::super::agent_effort::is_subagent_request(request) {
        developer_instructions.push_str("\n\n");
        developer_instructions.push_str(ORCHESTRATOR_INSTRUCTIONS);
    }
    let base_instructions = if system.is_empty() {
        developer_instructions.clone()
    } else {
        format!("{system}\n\n{developer_instructions}")
    };
    json!({
        "model": model,
        "cwd": cwd,
        "baseInstructions": base_instructions,
        "developerInstructions": developer_instructions,
        "dynamicTools": dynamic_tools,
        "environments": [],
        "ephemeral": true,
        "approvalPolicy": "never",
        // Codex built-in execution tools remain disabled below. Using workspace-write here
        // prevents the provider from misrepresenting Claude Code's dynamic tools as read-only.
        "sandbox": "workspace-write",
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
