use std::path::Path;

use tokio::process::Command;

use super::options::SubscriptionOptions;
use crate::NONINTERACTIVE_CHILD_ENV;

#[derive(Clone, Copy)]
pub(in crate::anthropic) enum OutputMode {
    Json,
    StreamJson,
}

pub(in crate::anthropic) fn subscription_command(
    program: &Path,
    model: &str,
    options: &SubscriptionOptions,
    output: OutputMode,
) -> Command {
    let mut command = Command::new(program);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.as_std_mut().process_group(0);
    }
    let output_format = match output {
        OutputMode::Json => "json",
        OutputMode::StreamJson => "stream-json",
    };
    command.args([
        "--print",
        "--model",
        model,
        "--output-format",
        output_format,
        "--no-session-persistence",
    ]);
    if options.disable_tools {
        command.args(["--safe-mode", "--tools", "", "--allowedTools", ""]);
    } else if !options.tools.is_empty() {
        let tools = options.tools.join(",");
        command.args(["--tools", &tools]);
        command.args(["--allowedTools", &tools]);
    }
    if let Some(schema) = &options.json_schema {
        command.args(["--json-schema", schema]);
    }
    if matches!(output, OutputMode::StreamJson) {
        command.args(["--include-partial-messages", "--verbose"]);
    }
    if let Some(effort) = &options.effort {
        command.args(["--effort", effort]);
    }
    if let Some(cwd) = &options.cwd {
        command.current_dir(cwd);
    }
    command.env(NONINTERACTIVE_CHILD_ENV, "1");
    remove_proxy_environment(&mut command);
    command
}

fn remove_proxy_environment(command: &mut Command) {
    for variable in [
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_MODEL",
        // Isolated claudex CLAUDE_CONFIG_DIR has no OAuth; subscription
        // children must use the real ~/.claude login instead.
        "CLAUDE_CONFIG_DIR",
        "CLAUDEX_ACTIVE",
        "CLAUDE_CODE_ENABLE_EXPERIMENTAL_ADVISOR_TOOL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
        "ENABLE_CLAUDEAI_MCP_SERVERS",
    ] {
        command.env_remove(variable);
    }
    crate::web_search::clear_local_ccr_environment(command);
}
