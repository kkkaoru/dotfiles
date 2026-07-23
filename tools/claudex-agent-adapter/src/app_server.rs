use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex, oneshot},
};

pub(crate) mod events;
use events::ThreadEventDispatcher;
pub use events::ThreadEvents;
mod pending;
use pending::{PendingRequest, PendingResponse, await_response};

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A persistent JSON-RPC connection to `codex app-server` over JSONL stdio.
pub struct AppServer {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, PendingResponse>>,
    event_dispatcher: ThreadEventDispatcher,
    alive: AtomicBool,
}

impl AppServer {
    pub async fn spawn(model: &str) -> Result<Arc<Self>> {
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        let source_home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(&home).join(".codex"));
        let isolated_home = PathBuf::from(home).join(".cache/claudex/codex-home");
        let program = std::env::var_os("CLAUDEX_CODEX_PROGRAM").unwrap_or_else(|| "codex".into());
        Self::spawn_with_program(model, program, &source_home, &isolated_home).await
    }

    pub async fn spawn_with_program(
        model: &str,
        program: impl AsRef<std::ffi::OsStr>,
        source_home: &std::path::Path,
        isolated_home: &std::path::Path,
    ) -> Result<Arc<Self>> {
        let codex_home = prepare_isolated_codex_home(source_home, isolated_home)?;
        let mut child = Command::new(program)
            .args([
                "app-server",
                "--stdio",
                "--disable",
                "shell_tool",
                "--disable",
                "unified_exec",
                "--disable",
                "web_search",
                "--disable",
                "tool_search",
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
            .env("CODEX_HOME", &codex_home)
            .env("RUST_LOG", "error")
            .current_dir(&codex_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .context("failed to start `codex app-server`")?;

        let stdin = child
            .stdin
            .take()
            .context("app-server stdin is unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("app-server stdout is unavailable")?;
        let server = Arc::new(Self {
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            event_dispatcher: ThreadEventDispatcher::default(),
            alive: AtomicBool::new(true),
        });

        tokio::spawn(Self::read_loop(Arc::clone(&server), stdout));
        let initialize = server
            .request_with_timeout("initialize", initialize_params(), INITIALIZE_TIMEOUT)
            .await
            .context("app-server initialization failed");
        if let Err(error) = initialize {
            server.stop("codex app-server initialization failed").await;
            return Err(error);
        }
        if let Err(error) = server
            .notify("initialized", json!({}))
            .await
            .context("failed to acknowledge app-server initialization")
        {
            server
                .stop("failed to acknowledge codex app-server initialization")
                .await;
            return Err(error);
        }
        Ok(server)
    }

    pub fn subscribe_thread(&self, thread_id: &str) -> ThreadEvents {
        self.event_dispatcher.subscribe(thread_id)
    }

    #[cfg(test)]
    pub(crate) fn dispatch_test_event(&self, event: Value) {
        self.event_dispatcher.dispatch(event);
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        self.request_with_timeout(method, params, REQUEST_TIMEOUT)
            .await
    }

    async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let request = self.begin_request(method, params).await?;
        match tokio::time::timeout(timeout, await_response(request.response)).await {
            Ok(response) => response,
            Err(_) => {
                self.pending.lock().await.remove(&request.id);
                bail!("app-server request `{method}` timed out after {timeout:?}")
            }
        }
    }

    /// Starts a request after flushing it to app-server, but does not delay the
    /// caller while app-server keeps the JSON-RPC response open for the turn.
    pub async fn request_detached(&self, method: &str, params: Value) -> Result<()> {
        let thread_id = params.get("threadId").cloned().unwrap_or(Value::Null);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending
            .lock()
            .await
            .insert(id, PendingResponse::Detached { thread_id });
        if let Err(error) = self
            .write(&json!({ "id": id, "method": method, "params": params }))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        Ok(())
    }

    async fn begin_request(&self, method: &str, params: Value) -> Result<PendingRequest> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .await
            .insert(id, PendingResponse::Awaited(tx));
        if let Err(error) = self
            .write(&json!({ "id": id, "method": method, "params": params }))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        Ok(PendingRequest { id, response: rx })
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.write(&json!({ "method": method, "params": params }))
            .await
    }

    pub async fn respond(&self, id: Value, result: Value) -> Result<()> {
        self.write(&json!({ "id": id, "result": result })).await
    }

    async fn write(&self, value: &Value) -> Result<()> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&line).await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn read_loop(server: Arc<Self>, stdout: tokio::process::ChildStdout) {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            let Some(line) = server.next_line(&mut lines).await else {
                return;
            };
            server.dispatch_line(&line).await;
        }
    }

    async fn next_line(
        &self,
        lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    ) -> Option<String> {
        match lines.next_line().await {
            Ok(Some(line)) => Some(line),
            Ok(None) => {
                self.stop("codex app-server exited or closed its output")
                    .await;
                None
            }
            Err(error) => {
                tracing::error!(%error, "failed to read codex app-server output");
                self.stop(&format!("failed to read codex app-server output: {error}"))
                    .await;
                None
            }
        }
    }

    async fn dispatch_line(&self, line: &str) {
        match serde_json::from_str::<Value>(line) {
            Ok(message) => self.dispatch(message).await,
            Err(error) => tracing::warn!(%error, %line, "invalid app-server JSONL message"),
        }
    }

    async fn stop(&self, reason: &str) {
        if !self.alive.swap(false, Ordering::Relaxed) {
            return;
        }
        self.fail_pending(reason).await;
        self.event_dispatcher.close();

        let mut child = self.child.lock().await;
        let status = match child.try_wait() {
            Ok(Some(status)) => Ok(status),
            Ok(None) => {
                let _ = child.start_kill();
                child.wait().await
            }
            Err(error) => Err(error),
        };
        tracing::error!(?status, %reason, "codex app-server stopped");
    }

    async fn dispatch(&self, message: Value) {
        if message.get("method").is_some() {
            self.event_dispatcher.dispatch(message);
            return;
        }

        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            tracing::debug!(
                ?message,
                "ignored app-server message without method or numeric id"
            );
            return;
        };
        let Some(tx) = self.pending.lock().await.remove(&id) else {
            tracing::debug!(id, "received response for unknown app-server request");
            return;
        };
        match tx {
            PendingResponse::Awaited(tx) => {
                let response = if let Some(error) = message.get("error") {
                    Err(error.to_string())
                } else {
                    Ok(message.get("result").cloned().unwrap_or(Value::Null))
                };
                let _ = tx.send(response);
            }
            PendingResponse::Detached { thread_id } => {
                if let Some(error) = message.get("error") {
                    self.dispatch_detached_error(thread_id, error);
                }
            }
        }
    }

    async fn fail_pending(&self, reason: &str) {
        for (_, response) in self.pending.lock().await.drain() {
            match response {
                PendingResponse::Awaited(tx) => {
                    let _ = tx.send(Err(reason.to_owned()));
                }
                PendingResponse::Detached { thread_id } => {
                    self.dispatch_detached_error(thread_id, &reason);
                }
            }
        }
    }

    fn dispatch_detached_error(&self, thread_id: Value, error: &dyn std::fmt::Display) {
        self.event_dispatcher.dispatch(json!({
            "method":"error",
            "params":{
                "threadId":thread_id,
                "willRetry":false,
                "error":{"message":format!("turn/start failed: {error}")}}
        }));
    }
}

fn initialize_params() -> Value {
    json!({
        "clientInfo": {
            "name": "claudex",
            "title": "claudex Anthropic compatibility adapter",
            "version": env!("CARGO_PKG_VERSION")
        },
        "capabilities": { "experimentalApi": true }
    })
}

fn prepare_isolated_codex_home(
    source_home: &std::path::Path,
    isolated: &std::path::Path,
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

    let _ = std::fs::set_permissions(
        isolated.join("auth.json"),
        std::fs::Permissions::from_mode(0o600),
    );

    // An isolated home prevents the Codex runtime from loading the user's MCP
    // servers, hooks, skills, and AGENTS instructions alongside Claude Code's
    // equivalent tools and context.
    std::fs::write(
        isolated.join("config.toml"),
        r#"web_search = "disabled"

[features]
apps = false
multi_agent = false
plugins = false
remote_control = false
shell_tool = false
tool_search = false
unified_exec = false
web_search = false
"#,
    )?;
    Ok(isolated.to_path_buf())
}

pub fn response_thread_id(value: &Value) -> Result<String> {
    value
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("thread/start response did not contain thread.id: {value}"))
}

#[cfg(test)]
include!("app_server_tests.rs");
