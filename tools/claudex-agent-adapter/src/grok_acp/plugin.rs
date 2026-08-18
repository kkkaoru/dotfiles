use std::{ffi::OsStr, fs, path::PathBuf};

use anyhow::{Context, Result};

pub(super) const ROUTING_INSTRUCTIONS: &str = "Claudex SubAgent routing on ACP: launch workers \
through MCP claudex-launch Agent or Task when that server is attached (preferred), or any other \
Agent/Task tool that Claude Code supplied. Prefer subagent_type from selected_workers \
(claudex-grok, claudex-cursor, claudex-fugu, …), set claudex_model and claudex_effort to that \
worker's exact model/effort, and set run_in_background=true unless the user requires a \
synchronous result. Do not use provider-native Task/Agent fan-out for Claudex workers: those \
finish inside Cursor/OpenCode/Grok and never become Claude Code SubAgents (▶ Task / ✓ Task text \
alone is not a launch). If only spawn_subagent is available, call it with description+prompt and \
the adapter bridges it to Claude Code Agent tool_use so the Claudex agents panel tracks the \
worker—do not wait for the provider-native child. These launch rules apply only when you are the \
Claudex main session, not a delegated worker. After a background launch, emit a short status \
and end the turn immediately; do not call get_command_or_subagent_output or TaskOutput with a \
positive timeout_ms in the same turn. Retrieve results only after a Claude completion notification \
on a later turn.";

const PROFILE_NAME: &str = "claudex-medium";
const PROFILE_EFFORT: &str = "medium";
/// Legacy profile kept for cache cleanup; Claudex no longer launches via spawn_subagent.
#[cfg(test)]
const QUALIFIED_PROFILE_NAME: &str = "grok-native-medium-plugin-v3:claudex-medium";
const UNSAFE_CROSS_PROVIDER_ALIASES: &[&str] = &[
    "custom-advisor",
    "claudex-orchestrator",
    "claudex-gpt",
    "claudex-gpt-spark",
    "claudex-grok",
    "claudex-deepseek-pro",
    "claudex-deepseek-flash",
    "claudex-cline-deepseek-flash",
    "claudex-haiku-search",
    "claudex-fugu",
    "claudex-sonnet",
    "claudex-ollama-glm-5-2",
    "claudex-qwen",
    "claudex-command-code",
    "claudex-command-code-muse-spark-1-2-contributor",
    "claudex-haiku",
];

/// Prefer Claude-visible Agent launches; still allow spawn_subagent so the adapter can bridge it.
const REJECT_UNSAFE_AGENT_SCRIPT: &str = r#"#!/bin/sh
if [ "${CLAUDEX_GROK_ACP:-}" != "1" ]; then
  printf '%s\n' '{"decision":"allow"}'
  exit 0
fi
# Allow spawn_subagent and Agent/Task: adapter bridges them to Claude Code tool_use.
printf '%s\n' '{"decision":"allow"}'
"#;

pub(super) fn prepare(program: &OsStr) -> Result<Option<PathBuf>> {
    prepare_with(
        program,
        std::env::var_os("CLAUDEX_GROK_PLUGIN_DIR").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

fn prepare_with(
    program: &OsStr,
    plugin_dir: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Result<Option<PathBuf>> {
    if let Some(path) = plugin_dir {
        return Ok(Some(path));
    }
    if PathBuf::from(program).file_name() != Some(OsStr::new("grok")) {
        return Ok(None);
    }
    let home = home.context("HOME is required for Grok plugin cache")?;
    let root = home.join(".cache/claudex/grok-native-medium-plugin-v3");
    let agents = root.join("agents");
    let bin = root.join("bin");
    let user_hooks = home.join(".grok/hooks");
    fs::create_dir_all(&agents).context("create Grok effort plugin cache")?;
    fs::create_dir_all(&bin).context("create Grok effort plugin commands")?;
    fs::create_dir_all(&user_hooks).context("create Grok user hooks")?;
    write_if_changed(
        agents.join(format!("{PROFILE_NAME}.md")),
        &profile(PROFILE_NAME),
    )?;
    for alias in UNSAFE_CROSS_PROVIDER_ALIASES {
        let stale_shadow = agents.join(format!("{alias}.md"));
        if stale_shadow.exists() {
            fs::remove_file(&stale_shadow)
                .with_context(|| format!("remove stale Grok shadow {}", stale_shadow.display()))?;
        }
    }
    let guard = bin.join("reject-cross-provider-agent.sh");
    write_if_changed(guard.clone(), REJECT_UNSAFE_AGENT_SCRIPT)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&guard)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&guard, permissions)
            .with_context(|| format!("make {} executable", guard.display()))?;
    }
    let stale_plugin_hook = root.join("hooks/hooks.json");
    if stale_plugin_hook.exists() {
        fs::remove_file(&stale_plugin_hook).context("remove ineffective process-plugin hook")?;
    }
    write_if_changed(
        user_hooks.join("claudex-agent-adapter.json"),
        &hooks_json(&guard)?,
    )?;
    Ok(Some(root))
}

fn hooks_json(guard: &PathBuf) -> Result<String> {
    let value = serde_json::json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "^spawn_subagent$",
                "hooks": [{
                    "type": "command",
                    "command": guard,
                    "timeout": 5
                }]
            }]
        }
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

fn write_if_changed(path: PathBuf, content: &str) -> Result<()> {
    if fs::read_to_string(&path).ok().as_deref() == Some(content) {
        return Ok(());
    }
    fs::write(&path, content).with_context(|| format!("write {}", path.display()))
}

fn profile(name: &str) -> String {
    format!(
        "---\nname: {name}\n\
         description: Provider-local Grok SubAgent inheriting the active model with medium reasoning effort.\n\
         promptMode: extend\neffort: {PROFILE_EFFORT}\n---\n\n\
         Work as a Grok-native general-purpose SubAgent. Inherit the active model, never select a \
         cross-provider agent, and complete the delegated task.\n"
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
