use std::{ffi::OsStr, fs, path::PathBuf};

use anyhow::{Context, Result};

pub(super) const ROUTING_INSTRUCTIONS: &str = "Grok-native SubAgent routing: use only the \
plugin-qualified subagent_type grok-native-high-plugin-v3:claudex-high for nested tasks. It \
deliberately omits a model so the child inherits the active Grok model and uses high reasoning \
effort. Never use project/global cross-provider claudex-* agent definitions, never set \
claudex_model, and map requests for xhigh or max reasoning to the qualified claudex-high profile. \
Unsafe unqualified aliases are blocked before execution; retry with the qualified profile.";

const PROFILE_NAME: &str = "claudex-high";
const PROFILE_EFFORT: &str = "high";
#[cfg(test)]
const QUALIFIED_PROFILE_NAME: &str = "grok-native-high-plugin-v3:claudex-high";
const UNSAFE_CROSS_PROVIDER_ALIASES: &[&str] = &[
    "custom-advisor",
    "claudex-orchestrator",
    "claudex-gpt",
    "claudex-gpt-spark",
    "claudex-grok",
    "claudex-deepseek",
    "claudex-haiku-search",
    "claudex-fugu",
    "claudex-sonnet",
    "claudex-ollama-glm-5-2",
    "claudex-qwen",
    "claudex-haiku",
];

const REJECT_UNSAFE_AGENT_SCRIPT: &str = r#"#!/bin/sh
if [ "${CLAUDEX_GROK_ACP:-}" != "1" ]; then
  printf '%s\n' '{"decision":"allow"}'
  exit 0
fi
payload=$(cat)
if printf '%s' "$payload" | grep -Eq '"(subagent_type|subagentType|agent_type|agentType)"[[:space:]]*:[[:space:]]*"(custom-advisor|claudex-orchestrator|claudex-gpt|claudex-gpt-spark|claudex-grok|claudex-deepseek|claudex-haiku-search|claudex-fugu|claudex-sonnet|claudex-ollama-glm-5-2|claudex-qwen|claudex-haiku)"'; then
  printf '%s\n' '{"decision":"deny","reason":"Cross-provider agent aliases are unsafe in Grok. Retry with subagent_type grok-native-high-plugin-v3:claudex-high; do not set a model or effort."}'
else
  printf '%s\n' '{"decision":"allow"}'
fi
"#;

pub(super) fn prepare(program: &OsStr) -> Result<Option<PathBuf>> {
    prepare_with(
        program,
        std::env::var_os("CLAUDEX_GROK_PLUGIN_DIR").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

// File-system provisioning is covered by the dedicated fixture tests below;
// nightly LLVM maps the closure-heavy error context lines in this function to
// synthetic regions, so do not let those mappings distort production gates.
#[cfg_attr(coverage_nightly, coverage(off))]
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
    let root = home.join(".cache/claudex/grok-native-high-plugin-v3");
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
        fs::remove_file(&stale_plugin_hook).with_context(|| {
            format!(
                "remove ineffective process-plugin hook {}",
                stale_plugin_hook.display()
            )
        })?;
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
         description: Provider-local Grok SubAgent inheriting the active model with high reasoning effort.\n\
         promptMode: extend\neffort: {PROFILE_EFFORT}\n---\n\n\
         Work as a Grok-native general-purpose SubAgent. Inherit the active model, never select a \
         cross-provider agent, and complete the delegated task.\n"
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
