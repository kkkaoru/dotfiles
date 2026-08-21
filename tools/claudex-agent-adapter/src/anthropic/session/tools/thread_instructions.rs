use super::super::super::MessagesRequest;

pub(in crate::anthropic) fn build_developer_instructions(
    request: &MessagesRequest,
    is_subagent: bool,
    acp_native: bool,
) -> String {
    let bridge = if acp_native {
        super::super::ACP_NATIVE_BRIDGE_INSTRUCTIONS
    } else {
        super::super::super::super::BRIDGE_INSTRUCTIONS
    };
    let mut developer_instructions = super::super::super::super::team_protocol::guidance(request)
        .map_or_else(
            || bridge.to_owned(),
            |guidance| format!("{bridge}\n\n{guidance}"),
        );
    if acp_native {
        developer_instructions.push_str("\n\n");
        developer_instructions
            .push_str(crate::anthropic::subscription_request::SHARED_WORKSPACE_INSTRUCTIONS);
        developer_instructions.push_str("\n\n");
        developer_instructions.push_str(super::super::WORKTREE_LIFECYCLE_INSTRUCTIONS);
        developer_instructions.push_str("\n\n");
        developer_instructions.push_str(super::super::WORKTREE_TARGET_INSTRUCTIONS);
        developer_instructions.push_str("\n\n");
        // ACP providers execute their own tools; forcing Claude Code Agent/Task causes silence.
        if is_subagent {
            developer_instructions.push_str(super::super::ACP_NATIVE_WORKER_INSTRUCTIONS);
            developer_instructions.push_str("\n\n");
            developer_instructions.push_str(super::super::SUBAGENT_MAIN_ONLY_TOOLS_INSTRUCTIONS);
        } else {
            developer_instructions.push_str(super::super::ACP_NATIVE_ORCHESTRATOR_INSTRUCTIONS);
        }
        return developer_instructions;
    }
    developer_instructions.push_str("\n\n");
    if is_subagent {
        // Main Codex/Terra orchestrators treat this as "do the code task yourself".
        developer_instructions
            .push_str(super::super::super::super::CODEX_APP_SERVER_PARALLELIZATION_INSTRUCTIONS);
        developer_instructions.push_str("\n\n");
    }
    developer_instructions
        .push_str(crate::anthropic::subscription_request::SHARED_WORKSPACE_INSTRUCTIONS);
    developer_instructions.push_str("\n\n");
    developer_instructions.push_str(super::super::WORKTREE_LIFECYCLE_INSTRUCTIONS);
    developer_instructions.push_str("\n\n");
    developer_instructions.push_str(super::super::WORKTREE_TARGET_INSTRUCTIONS);
    developer_instructions.push_str("\n\n");
    developer_instructions.push_str(super::super::super::super::SUBAGENT_RESULT_PROTOCOL);
    developer_instructions.push_str(
        "\n\nCommand execution is available to every routed worker. If Claude Code supplies a shell, Bash, unified-exec, or command tool, use it when the active task requires it; do not refuse an available command tool because the backend is Codex, Grok, OpenCode, or Cursor.",
    );
    if is_subagent {
        developer_instructions.push_str("\n\n");
        developer_instructions.push_str(super::super::SUBAGENT_MAIN_ONLY_TOOLS_INSTRUCTIONS);
        return developer_instructions;
    }
    developer_instructions.push_str("\n\n");
    developer_instructions.push_str(super::super::SUBAGENT_LIFECYCLE_INSTRUCTIONS);
    developer_instructions.push_str("\n\n");
    developer_instructions.push_str(super::super::ORCHESTRATOR_INSTRUCTIONS);
    // Turn-varying scheduler last, matching subscription prompt-cache order.
    developer_instructions.push_str("\n\n");
    developer_instructions.push_str(&super::super::parallel_scheduler_instructions(request));
    developer_instructions
}

pub(super) fn isolated_runtime_cwd() -> String {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    home.join(".cache/claudex/codex-home")
        .to_string_lossy()
        .into_owned()
}
