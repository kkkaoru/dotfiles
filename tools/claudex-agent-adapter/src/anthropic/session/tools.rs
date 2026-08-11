use std::collections::HashMap;

use serde_json::{Value, json};

use super::super::MessagesRequest;
use crate::agent_backend::WebSearchMode;
mod instructions;
pub(super) use instructions::*;
mod thread;
#[cfg(test)]
pub(in crate::anthropic) use thread::thread_start_params;
pub(in crate::anthropic) use thread::thread_start_params_for_mode;

#[cfg(test)]
pub(in crate::anthropic) fn tool_configuration(
    request: &MessagesRequest,
    advisor_model: Option<&str>,
    collaborator_model: Option<&str>,
) -> (Vec<Value>, HashMap<String, String>, HashMap<String, String>) {
    tool_configuration_for_mode(
        request,
        advisor_model,
        collaborator_model,
        WebSearchMode::default(),
    )
}

pub(in crate::anthropic) fn tool_configuration_for_mode(
    request: &MessagesRequest,
    _advisor_model: Option<&str>,
    _collaborator_model: Option<&str>,
    web_search_mode: WebSearchMode,
) -> (Vec<Value>, HashMap<String, String>, HashMap<String, String>) {
    let allow_team_messages = super::super::subagent_reuse::agent_teams_enabled(request);
    let (tools, external_names) = external_tools(
        &request.tools,
        web_search_mode,
        super::super::subagent_reuse::should_expose_launch_tools(request),
        allow_team_messages,
        super::super::agent_effort::is_subagent_request(request),
    );
    // Provider-side tools must be an exact projection of the schemas supplied by
    // Claude Code.  Advisor/collaborator work is therefore exposed only when
    // Claude Code sends a public tool schema; the adapter never synthesizes or
    // executes an invisible inference tool of its own.
    (tools, external_names, HashMap::new())
}

pub(in crate::anthropic) fn is_main_session_only_tool(name: &str) -> bool {
    let stem = name
        .strip_prefix("cc_")
        .unwrap_or(name)
        .split(['_', '-'])
        .next()
        .unwrap_or(name);
    stem.eq_ignore_ascii_case("advisor")
}

fn external_tools(
    tools: &[Value],
    web_search_mode: WebSearchMode,
    expose_launch_tools: bool,
    allow_team_messages: bool,
    hide_main_only_tools: bool,
) -> (Vec<Value>, HashMap<String, String>) {
    let mut specs = Vec::new();
    let mut names = HashMap::new();
    for (index, tool) in tools.iter().enumerate() {
        let Some(original_name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        if hide_main_only_tools && is_main_session_only_tool(original_name) {
            continue;
        }
        if web_search_mode == WebSearchMode::CodexNative && original_name == "WebSearch" {
            continue;
        }
        // Keep the original index when projecting Claude Code's dynamic tools.
        // The provider may return a tool call using that index, so filtering a
        // cloned list before enumerate() would route TaskUpdate/TaskOutput to
        // the wrong tool.
        if !allow_team_messages && original_name == "SendMessage" {
            continue;
        }
        if !expose_launch_tools && super::super::subagent_reuse::is_launch_tool(original_name) {
            tracing::warn!(
                tool = original_name,
                "hiding native SubAgent launch after session budget"
            );
            continue;
        }
        let codex_name = codex_tool_name(original_name, index);
        let spec = dynamic_tool(tool, &codex_name).expect("tool name was validated");
        names.insert(codex_name, original_name.to_owned());
        specs.push(spec);
    }
    (specs, names)
}

pub(in crate::anthropic) fn dynamic_tool(tool: &Value, codex_name: &str) -> Option<Value> {
    let original_name = tool.get("name")?.as_str()?;
    let lifecycle_guidance = task_lifecycle_guidance(original_name);
    Some(json!({
        "type": "function",
        "name": codex_name,
        "description": format!(
            "Claude Code tool `{original_name}`. {}{}",
            tool.get("description").and_then(Value::as_str).unwrap_or(""),
            lifecycle_guidance
        ),
        "inputSchema": tool.get("input_schema").cloned()
            .unwrap_or_else(|| json!({"type":"object"}))
    }))
}

fn task_lifecycle_guidance(tool_name: &str) -> &'static str {
    match tool_name {
        "Agent" | "Task" => {
            " When a compatible SubAgent already exists for this scope, set resume to that exact agentId instead of launching a replacement. Launch a new worker only for independent scope or a failed/cancelled/stopped prior worker."
        }
        "TaskStop" | "StopTask" | "Stop Task" => {
            " Task lifecycle: stopping is idempotent; use only the exact active Agent task_id from the current launch (`a` + 16 hex). Never guess IDs, never stop Bash-background nanoids (e.g. b13mjnjlj) or previous-session orphan IDs from `No completion record` notifications, and never cascade stops onto unrelated in-flight workers after one lane fails. When the user asks to stop remaining session SubAgents or leftover cards remain after a mid-response API/ACP error, TaskStop every live `a`+16-hex id from TaskList in the same turn; do not inspect OS processes or kill the claudex serve daemon. A `No task found` response means already stopped/completed; do not retry. ACP unavailable or dropped response is also already stopped."
        }
        "TaskOutput" | "TaskGet" => {
            " TaskOutput: use only the exact task_id from that Agent/Task launch result (`a` + 16 hex). Never guess, never pass a display name or agentId unless the launch result said it is the TaskOutput task_id, and never reuse a previous-session orphan. If Claude Code returns `No task found` and lists `Running background agents`, the ID was wrong — retry with one of those live ids. That miss is not completed output."
        }
        _ => "",
    }
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
