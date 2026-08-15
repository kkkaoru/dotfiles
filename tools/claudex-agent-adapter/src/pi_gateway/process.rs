use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::{
    net::UnixStream,
    process::{Child, Command},
};
use uuid::Uuid;

const START_TIMEOUT: Duration = Duration::from_secs(15);
const START_POLL_INTERVAL: Duration = Duration::from_millis(25);
const PI_PROGRAM_ENV: &str = "CLAUDEX_PI_PROGRAM";
const PI_EXTENSION_ENV: &str = "CLAUDEX_PI_EXTENSION";

pub(super) struct GatewayProcess {
    pub(super) child: Child,
    pub(super) directory: PathBuf,
    pub(super) socket: PathBuf,
    pub(super) token: String,
}

impl GatewayProcess {
    pub(super) async fn spawn(extensions: &[String]) -> Result<Self> {
        validate_isolated_extensions(extensions)?;
        let directory = runtime_directory()?;
        let socket = directory.join("gateway.sock");
        let token = Uuid::new_v4().simple().to_string();
        let program = std::env::var_os(PI_PROGRAM_ENV).unwrap_or_else(|| "pi".into());
        let mut command = Command::new(program);
        #[cfg(unix)]
        command.process_group(0);
        command
            .args([
                "--mode",
                "rpc",
                "--no-session",
                "--no-context-files",
                "--no-tools",
                "--offline",
            ])
            .env("CLAUDEX_PI_GATEWAY_SOCKET", &socket)
            .env("CLAUDEX_PI_GATEWAY_TOKEN", &token)
            .env("CLAUDEX_PI_GATEWAY_ORIGIN", "claudex")
            .env("PI_CODING_AGENT_DIR", isolate_agent_directory(&directory)?)
            .envs(exa_api_key().map(|key| ("EXA_API_KEY", key)))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if extensions.is_empty() {
            append_ambient_extensions(&mut command, std::env::var_os(PI_EXTENSION_ENV));
        } else {
            append_isolated_extensions(&mut command, extensions);
        }
        let child = command.spawn().context("failed to start Pi gateway")?;
        let process = Self {
            child,
            directory,
            socket,
            token,
        };
        if let Err(error) = process.wait_until_connectable().await {
            process.shutdown().await;
            return Err(error);
        }
        Ok(process)
    }

    async fn wait_until_connectable(&self) -> Result<()> {
        tokio::time::timeout(START_TIMEOUT, wait_for_socket(&self.socket))
            .await
            .with_context(|| {
                format!(
                    "Pi gateway did not create {} within {}s",
                    self.socket.display(),
                    START_TIMEOUT.as_secs()
                )
            })
    }

    pub(super) async fn shutdown(mut self) {
        drop(self.child.stdin.take());
        let wait = tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await;
        if wait.is_err() {
            let _ = self.child.kill().await;
            let _ = self.child.wait().await;
        }
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_dir(&self.directory);
    }
}

fn validate_isolated_extensions(extensions: &[String]) -> Result<()> {
    for extension in extensions {
        let path = Path::new(extension);
        if !path.is_file() {
            anyhow::bail!(
                "Pi route extension is missing or not a file: {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn append_isolated_extensions(command: &mut Command, extensions: &[String]) {
    command.args([
        "--no-extensions",
        "--no-skills",
        "--no-prompt-templates",
        "--no-themes",
    ]);
    for extension in extensions {
        command.arg("--extension").arg(extension);
    }
}

fn append_ambient_extensions(command: &mut Command, extensions: Option<std::ffi::OsString>) {
    let Some(extensions) = extensions else {
        return;
    };
    for extension in std::env::split_paths(&extensions) {
        command.arg("--extension").arg(extension);
    }
}

async fn wait_for_socket(socket: &Path) {
    while !socket_is_connectable(socket).await {
        tokio::time::sleep(START_POLL_INTERVAL).await;
    }
}

async fn socket_is_connectable(socket: &Path) -> bool {
    UnixStream::connect(socket).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_extensions_disable_ambient_pi_resources() {
        let mut command = Command::new("pi");
        append_isolated_extensions(
            &mut command,
            &["/gateway.ts".to_owned(), "/provider.ts".to_owned()],
        );
        let arguments = command
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            [
                "--no-extensions",
                "--no-skills",
                "--no-prompt-templates",
                "--no-themes",
                "--extension",
                "/gateway.ts",
                "--extension",
                "/provider.ts",
            ]
        );
    }

    #[test]
    fn isolated_extension_validation_is_route_local() {
        let root = tempfile::tempdir().expect("extension fixture");
        let extension = root.path().join("gateway.ts");
        std::fs::write(&extension, "export default {};").expect("extension file");
        assert!(validate_isolated_extensions(&[extension.to_string_lossy().into_owned()]).is_ok());
        assert!(
            validate_isolated_extensions(&[root
                .path()
                .join("missing.ts")
                .to_string_lossy()
                .into_owned()])
            .is_err()
        );
        assert!(
            validate_isolated_extensions(&[root.path().to_string_lossy().into_owned()]).is_err()
        );
    }

    #[test]
    fn isolated_agent_directory_writes_empty_local_settings() {
        let runtime = tempfile::tempdir().expect("runtime fixture");
        let isolated = isolate_agent_directory(runtime.path()).expect("isolate");
        let isolated_settings = std::fs::read_to_string(isolated.join("settings.json"))
            .expect("read isolated settings");
        assert_eq!(isolated_settings, "{\n  \"packages\": []\n}\n");
        let isolated_search = std::fs::read_to_string(isolated.join("web-search.json"))
            .expect("read isolated web-search");
        assert_eq!(
            isolated_search,
            "{\n  \"exaApiKey\": \"$EXA_API_KEY\",\n  \"provider\": \"exa\",\n  \"workflow\": \"none\"\n}\n"
        );
        assert!(!isolated_search.contains("exa-"));
        assert_eq!(isolated, runtime.path().join("agent"));
    }
}

fn exa_api_key() -> Option<String> {
    if let Ok(key) = std::env::var("EXA_API_KEY") {
        let key = key.trim().to_owned();
        if !key.is_empty() {
            return Some(key);
        }
    }
    let home = std::env::var_os("HOME")?;
    for relative in [".codex/.env", ".env"] {
        let path = PathBuf::from(&home).join(relative);
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        for line in contents.lines() {
            let Some(value) = line.strip_prefix("EXA_API_KEY=") else {
                continue;
            };
            let value = value
                .trim()
                .trim_matches(|mark| mark == '"' || mark == '\'')
                .to_owned();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn isolate_agent_directory(runtime_directory: &Path) -> Result<PathBuf> {
    let agent_directory = runtime_directory.join("agent");
    fs::create_dir(&agent_directory).with_context(|| {
        format!(
            "create isolated Pi agent directory {}",
            agent_directory.display()
        )
    })?;
    set_private_permissions(&agent_directory)?;
    fs::write(
        agent_directory.join("settings.json"),
        "{\n  \"packages\": []\n}\n",
    )
    .with_context(|| {
        format!(
            "write isolated Pi settings {}",
            agent_directory.join("settings.json").display()
        )
    })?;
    fs::write(
        agent_directory.join("web-search.json"),
        "{\n  \"exaApiKey\": \"$EXA_API_KEY\",\n  \"provider\": \"exa\",\n  \"workflow\": \"none\"\n}\n",
    )
    .with_context(|| {
        format!(
            "write isolated Pi web-search config {}",
            agent_directory.join("web-search.json").display()
        )
    })?;
    if let Some(home) = std::env::var_os("HOME") {
        let source = PathBuf::from(home).join(".pi/agent");
        for name in ["auth.json", "models.json", "models-store.json"] {
            let from = source.join(name);
            if from.exists() {
                std::os::unix::fs::symlink(&from, agent_directory.join(name)).with_context(
                    || format!("share Pi {} with isolated agent directory", from.display()),
                )?;
            }
        }
    }
    Ok(agent_directory)
}

fn runtime_directory() -> Result<PathBuf> {
    #[cfg(unix)]
    let base = PathBuf::from("/tmp");
    #[cfg(not(unix))]
    let base = std::env::temp_dir();
    let directory = base.join(format!("claudex-pi-{}", Uuid::new_v4().simple()));
    fs::create_dir(&directory).with_context(|| {
        format!(
            "create Pi gateway runtime directory {}",
            directory.display()
        )
    })?;
    set_private_permissions(&directory)?;
    Ok(directory)
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("secure Pi gateway runtime directory {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
