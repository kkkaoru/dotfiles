use serde_json::{Value, json};

use super::super::super::{MessagesRequest, content::system_text};
use crate::agent_backend::WebSearchMode;
use crate::anthropic::subscription_request::cwd_from_system;

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
    let mut developer_instructions = super::super::super::team_protocol::guidance(&request.tools)
        .map_or_else(
            || super::super::super::BRIDGE_INSTRUCTIONS.to_owned(),
            |guidance| format!("{}\n\n{guidance}", super::super::super::BRIDGE_INSTRUCTIONS),
        );
    developer_instructions.push_str("\n\n");
    developer_instructions
        .push_str(super::super::super::CODEX_APP_SERVER_PARALLELIZATION_INSTRUCTIONS);
    developer_instructions.push_str("\n\n");
    developer_instructions
        .push_str(crate::anthropic::subscription_request::SHARED_WORKSPACE_INSTRUCTIONS);
    developer_instructions.push_str("\n\n");
    developer_instructions.push_str(&super::parallel_scheduler_instructions(request));
    developer_instructions.push_str("\n\n");
    developer_instructions.push_str(super::SUBAGENT_LIFECYCLE_INSTRUCTIONS);
    developer_instructions.push_str(
        "\n\nCommand execution is available to every routed worker. If Claude Code supplies a shell, Bash, unified-exec, or command tool, use it when the active task requires it; do not refuse an available command tool because the backend is Codex, Grok, or OpenCode.",
    );
    if !super::super::super::agent_effort::is_subagent_request(request) {
        developer_instructions.push_str("\n\n");
        developer_instructions.push_str(super::ORCHESTRATOR_INSTRUCTIONS);
    }
    let base_instructions = if system.is_empty() {
        developer_instructions.clone()
    } else {
        format!("{system}\n\n{developer_instructions}")
    };
    let web_search = if web_search_mode == WebSearchMode::CodexNative {
        "live"
    } else {
        "disabled"
    };
    let web_search_enabled = web_search_mode == WebSearchMode::CodexNative;
    json!({
        "model": model,
        "cwd": cwd,
        "baseInstructions": base_instructions,
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
        "config": {
            "web_search": web_search,
            "features": {
                "apps": false, "multi_agent": false, "shell_tool": true,
                "tool_search": true, "unified_exec": true, "web_search": web_search_enabled
            }
        }
    })
}

fn isolated_runtime_cwd() -> String {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    home.join(".cache/claudex/codex-home")
        .to_string_lossy()
        .into_owned()
}
