#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::process::{Child, Command};

use super::{isolated_config, provider_environment};

pub(super) fn initialize_params() -> Value {
    json!({
        "clientInfo": {
            "name": "claudex",
            "title": "claudex Anthropic compatibility adapter",
            "version": env!("CARGO_PKG_VERSION")
        },
        "capabilities": { "experimentalApi": true }
    })
}

pub(super) fn spawn_child(
    model: &str,
    program: impl AsRef<std::ffi::OsStr>,
    source_home: &Path,
    codex_home: &Path,
) -> Result<Child> {
    let mut command = Command::new(program);
    #[cfg(unix)]
    command.process_group(0);
    command
        .args([
            "app-server",
            "--stdio",
            "--disable",
            "apps",
            "--disable",
            "multi_agent",
            "--disable",
            "plugins",
            "--disable",
            "remote_control",
            "-c",
            &format!("model={model:?}"),
            "-c",
            "web_search=\"disabled\"",
        ])
        .env("CODEX_HOME", codex_home)
        .envs(provider_environment::credentials(source_home, codex_home))
        .env("RUST_LOG", "error")
        .current_dir(codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .context("failed to start `codex app-server`")
}

pub(super) fn prepare_isolated_codex_home(
    source_home: &Path,
    isolated: &Path,
) -> Result<PathBuf> {
    std::fs::create_dir_all(isolated)?;

    let source_auth = source_home.join("auth.json");
    if !source_auth.is_file() {
        bail!(
            "Codex authentication was not found at {}; run `codex login` first",
            source_auth.display()
        );
    }
    std::fs::copy(&source_auth, isolated.join("auth.json"))
        .with_context(|| format!("failed to copy {}", source_auth.display()))?;

    #[cfg(unix)]
    let _ = std::fs::set_permissions(
        isolated.join("auth.json"),
        std::fs::Permissions::from_mode(0o600),
    );

    // An isolated home prevents the Codex runtime from loading the user's MCP
    // servers, hooks, skills, and AGENTS instructions alongside Claude Code's
    // equivalent tools and context.
    let mut config = String::from(
        r#"web_search = "disabled"

[features]
apps = false
multi_agent = false
plugins = false
remote_control = false
shell_tool = true
tool_search = true
unified_exec = true
web_search = true
"#,
    );
    isolated_config::append_model_providers(source_home, &mut config)?;
    std::fs::write(isolated.join("config.toml"), config)?;
    Ok(isolated.to_path_buf())
}

pub fn response_thread_id(value: &Value) -> Result<String> {
    value
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("thread/start response did not contain thread.id: {value}"))
}
