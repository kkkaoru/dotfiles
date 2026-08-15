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
    pub(super) async fn spawn() -> Result<Self> {
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
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        append_extensions(&mut command, std::env::var_os(PI_EXTENSION_ENV));
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

fn append_extensions(command: &mut Command, extensions: Option<std::ffi::OsString>) {
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
