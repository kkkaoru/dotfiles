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
        // The daemon may close inherited descriptors, making stderr EBADF.
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("failed to start `codex app-server`")
}

pub(super) fn prepare_isolated_codex_home(source_home: &Path, isolated: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(isolated)?;
    let source_home = isolated_config::effective_source_home(source_home, isolated);

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
    isolated_config::prune_runtime_logs(isolated);
    isolated_config::append_model_providers(&source_home, &mut config)?;
    isolated_config::append_model_catalog(&source_home, &mut config)?;
    std::fs::write(isolated.join("config.toml"), config)?;
    Ok(isolated.to_path_buf())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod prepare_isolated_home_tests {
    use super::*;

    static HOME_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct HomeGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        home: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn push() -> Self {
            let lock = HOME_ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self {
                _lock: lock,
                home: std::env::var_os("HOME"),
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.home {
                Some(home) => unsafe { std::env::set_var("HOME", home) },
                None => unsafe { std::env::remove_var("HOME") },
            }
        }
    }

    #[test]
    fn production_isolated_home_copies_user_codex_providers_instead_of_stub_source() {
        let _guard = HomeGuard::push();
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let user_codex = home.join(".codex");
        let isolated = home.join(".cache/claudex/codex-home");
        std::fs::create_dir_all(&user_codex).unwrap();
        std::fs::create_dir_all(&isolated).unwrap();
        std::fs::write(
            user_codex.join("auth.json"),
            r#"{"tokens":{"access":"real"}}"#,
        )
        .unwrap();
        std::fs::write(
            user_codex.join("config.toml"),
            r#"model_catalog_json = "~/.codex/fugu.json"

[model_providers.sakana]
name = "Sakana"
base_url = "https://api.sakana.ai/v1"
env_key = "SAKANA_AI_PRO_API_KEY"
wire_api = "responses"
"#,
        )
        .unwrap();
        std::fs::write(isolated.join("logs_2.sqlite"), "stale").unwrap();
        let stub = root.path().join("stub-codex");
        std::fs::create_dir(&stub).unwrap();
        std::fs::write(stub.join("auth.json"), "{}").unwrap();
        unsafe {
            std::env::set_var("HOME", &home);
        }

        let prepared = prepare_isolated_codex_home(&stub, &isolated).unwrap();
        assert_eq!(prepared, isolated);
        assert_eq!(
            std::fs::read_to_string(isolated.join("auth.json")).unwrap(),
            r#"{"tokens":{"access":"real"}}"#
        );
        let config = std::fs::read_to_string(isolated.join("config.toml")).unwrap();
        assert!(config.contains("[model_providers.sakana]"));
        assert!(config.contains("model_catalog_json = \"~/.codex/fugu.json\""));
        assert!(!isolated.join("logs_2.sqlite").exists());
    }
}

pub fn response_thread_id(value: &Value) -> Result<String> {
    value
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("thread/start response did not contain thread.id: {value}"))
}
