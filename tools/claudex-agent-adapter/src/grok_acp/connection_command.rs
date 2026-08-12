use std::{env, ffi::OsString, path::Path, process::Stdio};

use anyhow::{Context as _, Result, bail};
use tokio::process::Command;

use super::super::plugin;
use super::AcpProvider;
use crate::path_env;

/// OpenCode's default `build` agent spawns nested explore/general task subagents. Under Claudex
/// those children inherit max effort on the same model and compete with the parent stream, so
/// Claude Code sees almost no output for minutes while OpenCode burns many internal steps.
/// Runtime config disables task/subagent fan-out only for this ACP child process.
pub(in crate::grok_acp) const OPENCODE_ACP_RUNTIME_CONFIG: &str = r#"{"subagent_depth":0,"permission":{"task":"deny"},"agent":{"build":{"permission":{"task":"deny"}}}}"#;

pub(in crate::grok_acp) fn build_provider_command(
    program: &OsString,
    provider: AcpProvider,
    arguments: Option<&[String]>,
    model: &str,
    effort: Option<&str>,
) -> Result<Command> {
    let mut command = Command::new(program);
    // Provider ACP children must not re-run Claude Code SessionStart / routing
    // hooks (Herdr stdin, capacity probes). Grok historically used CLAUDEX_GROK_ACP;
    // all ACP providers now also set CLAUDEX_PROVIDER_ACP for a single guard.
    command.env("CLAUDEX_PROVIDER_ACP", "1");
    match provider {
        AcpProvider::Grok => {
            let effort = effort.context("Grok reasoning effort is required at launch")?;
            if !matches!(effort, "low" | "medium" | "high") {
                bail!("Grok reasoning effort must be one of low, medium, or high");
            }
            command.env("CLAUDEX_GROK_ACP", "1");
            command.args([
                "--model",
                model,
                "--reasoning-effort",
                effort,
                "agent",
                "--always-approve",
                "--no-leader",
            ]);
            if let Some(path) = plugin::prepare(program)? {
                command.arg("--plugin-dir").arg(path);
            }
            command.arg("stdio");
        }
        AcpProvider::Copilot => {
            command.args(["--acp", "--stdio", "--model", model]);
        }
        AcpProvider::Configured | AcpProvider::ConfiguredLaunchScoped => {
            let arguments = arguments.context("configured ACP arguments are required")?;
            apply_opencode_acp_runtime_config(&mut command, program);
            for argument in arguments {
                command.arg(substitute_configured_argument(argument, model, effort)?);
            }
        }
    }
    command.env("PATH", path_env::tool_search_path());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.as_std_mut().process_group(0);
    }
    Ok(command)
}

pub(in crate::grok_acp) fn substitute_configured_argument(
    argument: &str,
    model: &str,
    effort: Option<&str>,
) -> Result<String> {
    let mut rendered = argument.replace("{model}", model);
    if rendered.contains("{effort}") {
        let effort = effort.context("configured ACP `{effort}` requires launch effort")?;
        rendered = rendered.replace("{effort}", normalize_launch_effort(effort));
    }
    Ok(rendered)
}

/// Map Claudex effort aliases onto values accepted by launch-scoped CLIs such as
/// Cline `--thinking` (`none|low|medium|high|xhigh`).
pub(in crate::grok_acp) fn normalize_launch_effort(effort: &str) -> &str {
    match effort {
        "mid" => "medium",
        "max" => "xhigh",
        other => other,
    }
}

pub(in crate::grok_acp) fn is_opencode_program(program: &OsString) -> bool {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "opencode" || name.starts_with("opencode-"))
}

#[cfg(test)]
pub(in crate::grok_acp) fn opencode_acp_runtime_config() -> &'static str {
    OPENCODE_ACP_RUNTIME_CONFIG
}

pub(in crate::grok_acp) fn apply_opencode_acp_runtime_config(
    command: &mut Command,
    program: &OsString,
) {
    if !is_opencode_program(program) {
        return;
    }
    // Leave an explicit user override alone; otherwise inject Claudex's anti-nesting policy.
    if env::var_os("OPENCODE_CONFIG_CONTENT").is_some() {
        return;
    }
    command.env("OPENCODE_CONFIG_CONTENT", OPENCODE_ACP_RUNTIME_CONFIG);
}

pub(in crate::grok_acp) fn spawn_provider_process(
    mut command: Command,
    provider: AcpProvider,
    cwd: &Path,
) -> Result<(tokio::process::Child, u32)> {
    let child = command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("start {} ACP server", provider.label()))?;
    let process_group = child
        .id()
        .ok_or_else(|| anyhow::anyhow!("{} ACP process id is unavailable", provider.label()))?;
    Ok((child, process_group))
}

pub(in crate::grok_acp) fn terminate_process_group(process_group: u32) {
    #[cfg(unix)]
    {
        let Ok(process_group) = i32::try_from(process_group) else {
            return;
        };
        // Signal the process group directly. Spawning a `kill` helper here can
        // itself be stranded when the adapter is shutting down, and a direct
        // SIGKILL also handles descendants that ignore TERM.
        let _ = unsafe { libc::kill(-process_group, libc::SIGKILL) };
    }
    #[cfg(not(unix))]
    let _ = process_group;
}

/// Owns a provider child for the complete driver lifetime. The async driver
/// normally performs an explicit reap, but this Drop guard is the last line of
/// defence when a LocalSet task or driver thread is aborted unexpectedly.
pub(in crate::grok_acp) struct ProviderChild {
    pub(in crate::grok_acp) child: tokio::process::Child,
    process_group: u32,
}

impl ProviderChild {
    pub(in crate::grok_acp) fn new(child: tokio::process::Child, process_group: u32) -> Self {
        Self {
            child,
            process_group,
        }
    }

    pub(in crate::grok_acp) async fn terminate_and_wait(
        &mut self,
    ) -> std::io::Result<std::process::ExitStatus> {
        terminate_process_group(self.process_group);
        let _ = self.child.start_kill();
        self.child.wait().await
    }
}

impl Drop for ProviderChild {
    fn drop(&mut self) {
        terminate_process_group(self.process_group);
        let _ = self.child.start_kill();
    }
}
