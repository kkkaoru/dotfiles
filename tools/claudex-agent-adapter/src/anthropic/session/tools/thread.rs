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
    let command_code = crate::command_code_acp::is_command_code_model(model);
    let launch_owner = super::super::super::request_identity::claude_session_id(request);
    let is_subagent = super::super::super::agent_effort::is_subagent_request(request);
    let acp_role = if is_subagent {
        "worker"
    } else {
        "orchestrator"
    };
    if command_code {
        // Muse Spark headless greets / reconstructs dirty git when Claudex
        // ACP_NATIVE dumps land in `cmd -p`. Keep cwd only; the ACP shim slims
        // the delegated task itself.
        return command_code_thread_start_params(
            model,
            &cwd,
            dynamic_tools,
            &launch_owner,
            acp_role,
        );
    }
    let acp_native = web_search_mode.uses_provider_native_agent_loop();
    let developer_instructions = build_developer_instructions(request, is_subagent, acp_native);
    let base_instructions = if system.is_empty() {
        developer_instructions.clone()
    } else {
        format!("{system}\n\n{developer_instructions}")
    };
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

fn command_code_thread_start_params(
    model: &str,
    cwd: &str,
    dynamic_tools: Vec<Value>,
    launch_owner: &Option<String>,
    acp_role: &str,
) -> Value {
    json!({
        "model": model,
        "cwd": cwd,
        "baseInstructions": "",
        "developerInstructions": "",
        "dynamicTools": dynamic_tools,
        "environments": [],
        "ephemeral": true,
        "approvalPolicy": "never",
        "sandbox": "danger-full-access",
        "personality": "none",
        "claudexLaunchOwner": launch_owner,
        "claudexAcpRole": acp_role,
        "config": {
            "web_search": "disabled",
            "features": {
                "apps": false, "multi_agent": false, "shell_tool": true,
                "tool_search": true, "unified_exec": true, "web_search": false
            }
        }
    })
}

fn build_developer_instructions(
    request: &MessagesRequest,
    is_subagent: bool,
    acp_native: bool,
) -> String {
    let bridge = if acp_native {
        super::ACP_NATIVE_BRIDGE_INSTRUCTIONS
    } else {
        super::super::super::BRIDGE_INSTRUCTIONS
    };
    let mut developer_instructions = super::super::super::team_protocol::guidance(request)
        .map_or_else(
            || bridge.to_owned(),
            |guidance| format!("{bridge}\n\n{guidance}"),
        );
    if acp_native {
        developer_instructions.push_str("\n\n");
        developer_instructions
            .push_str(crate::anthropic::subscription_request::SHARED_WORKSPACE_INSTRUCTIONS);
        developer_instructions.push_str("\n\n");
        // ACP providers execute their own tools; forcing Claude Code Agent/Task causes silence.
        if is_subagent {
            developer_instructions.push_str(super::ACP_NATIVE_WORKER_INSTRUCTIONS);
            developer_instructions.push_str("\n\n");
            developer_instructions.push_str(super::SUBAGENT_MAIN_ONLY_TOOLS_INSTRUCTIONS);
        } else {
            developer_instructions.push_str(super::ACP_NATIVE_ORCHESTRATOR_INSTRUCTIONS);
        }
        return developer_instructions;
    }
    developer_instructions.push_str("\n\n");
    if is_subagent {
        // Main Codex/Terra orchestrators treat this as "do the code task yourself".
        developer_instructions
            .push_str(super::super::super::CODEX_APP_SERVER_PARALLELIZATION_INSTRUCTIONS);
        developer_instructions.push_str("\n\n");
    }
    developer_instructions
        .push_str(crate::anthropic::subscription_request::SHARED_WORKSPACE_INSTRUCTIONS);
    developer_instructions.push_str("\n\n");
    developer_instructions.push_str(&super::parallel_scheduler_instructions(request));
    developer_instructions.push_str("\n\n");
    developer_instructions.push_str(super::SUBAGENT_LIFECYCLE_INSTRUCTIONS);
    developer_instructions.push_str("\n\n");
    developer_instructions.push_str(super::super::super::SUBAGENT_RESULT_PROTOCOL);
    developer_instructions.push_str(
        "\n\nCommand execution is available to every routed worker. If Claude Code supplies a shell, Bash, unified-exec, or command tool, use it when the active task requires it; do not refuse an available command tool because the backend is Codex, Grok, OpenCode, or Cursor.",
    );
    if is_subagent {
        developer_instructions.push_str("\n\n");
        developer_instructions.push_str(super::SUBAGENT_MAIN_ONLY_TOOLS_INSTRUCTIONS);
    } else {
        developer_instructions.push_str("\n\n");
        developer_instructions.push_str(super::ORCHESTRATOR_INSTRUCTIONS);
    }
    developer_instructions
}

fn isolated_runtime_cwd() -> String {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    home.join(".cache/claudex/codex-home")
        .to_string_lossy()
        .into_owned()
}
