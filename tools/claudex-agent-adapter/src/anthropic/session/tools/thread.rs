use serde_json::{Value, json};

use super::super::super::{MessagesRequest, content::system_text};
use crate::agent_backend::WebSearchMode;
use crate::anthropic::subscription_request::cwd_from_system;

#[path = "thread_instructions.rs"]
mod instructions;
pub(in crate::anthropic) use instructions::build_developer_instructions;
use instructions::isolated_runtime_cwd;

#[cfg(test)]
pub(in crate::anthropic) fn thread_start_params(
    request: &MessagesRequest,
    model: &str,
    dynamic_tools: Vec<Value>,
) -> Value {
    thread_start_params_for_mode(request, model, dynamic_tools, WebSearchMode::default())
}

pub(in crate::anthropic) fn thread_start_params_for_mode(
    request: &MessagesRequest,
    model: &str,
    dynamic_tools: Vec<Value>,
    web_search_mode: WebSearchMode,
) -> Value {
    let system = system_text(&request.system);
    let cwd = request
        .working_directory
        .clone()
        .or_else(|| cwd_from_system(&system))
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(isolated_runtime_cwd);
    let launch_owner = super::super::super::request_identity::claude_session_id(request);
    let is_subagent = super::super::super::agent_effort::is_subagent_request(request);
    let acp_role = if is_subagent {
        "worker"
    } else {
        "orchestrator"
    };
    let acp_native = web_search_mode.uses_provider_native_agent_loop();
    // Codex threadStart: baseInstructions is Claude system only. Developer
    // corpus is sent once in developerInstructions (not copied into base).
    let developer_instructions = build_developer_instructions(request, is_subagent, acp_native);
    let web_search_enabled = web_search_mode == WebSearchMode::CodexNative
        && request
            .tools
            .iter()
            .any(|tool| tool.get("name").and_then(Value::as_str) == Some("WebSearch"));
    let web_search = if web_search_enabled {
        "live"
    } else {
        "disabled"
    };
    json!({
        "model": model,
        "cwd": cwd,
        "baseInstructions": system,
        "developerInstructions": developer_instructions,
        "dynamicTools": dynamic_tools,
        "environments": [],
        "ephemeral": true,
        // Routed workers must retain the same command capability as Claude Code.  The adapter
        // still supplies the dynamic tool schemas, but must not silently downgrade shell access
        // when a model is handled by the Codex app-server backend.
        "approvalPolicy": "never",
        "sandbox": "danger-full-access",
        "personality": "none",
        "claudexLaunchOwner": launch_owner,
        "claudexAcpRole": acp_role,
        "config": {
            "web_search": web_search,
            "features": {
                "apps": false, "multi_agent": false, "shell_tool": true,
                "tool_search": true, "unified_exec": true, "web_search": web_search_enabled
            }
        }
    })
}

pub(in crate::anthropic) fn system_with_developer_instructions(
    system: &str,
    developer_instructions: &str,
) -> String {
    if system.is_empty() {
        developer_instructions.to_owned()
    } else {
        format!("{system}\n\n{developer_instructions}")
    }
}
