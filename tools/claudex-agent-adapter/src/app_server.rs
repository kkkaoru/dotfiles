use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin},
    sync::{Mutex, oneshot},
};

pub(crate) mod events;
use events::ThreadEventDispatcher;
pub use events::ThreadEvents;
mod codex_config;
pub(crate) use codex_config::{
    CODEX_CONFIG_FINGERPRINT_ENV, provider_config_fingerprint, source_home,
};
mod isolated_config;
mod lifecycle;
mod pending;
mod protocol;
mod provider_environment;
mod spawn;
use pending::{PendingRequest, PendingResponse, await_response};
pub use spawn::response_thread_id;
use spawn::{initialize_params, prepare_isolated_codex_home, spawn_child};

#[cfg(not(coverage_nightly))]
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(coverage_nightly)]
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(45);
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
        let source_home = source_home()?;
        let home = std::env::var_os("HOME").context("HOME is not set")?;
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
        let mut child = spawn_child(model, program, source_home, &codex_home)?;
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

        // A weak reader handle lets kill_on_drop stop an abandoned app-server process.
        tokio::spawn(Self::read_loop(Arc::downgrade(&server), stdout));
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

    pub async fn shutdown(&self) {
        self.stop("adapter shutdown").await;
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

    async fn read_loop(server: Weak<Self>, stdout: tokio::process::ChildStdout) {
        let mut lines = BufReader::new(stdout).lines();
        while Self::dispatch_next_line(&server, &mut lines).await {}
    }

    async fn dispatch_next_line(
        server: &Weak<Self>,
        lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    ) -> bool {
        let Some(line) = protocol::next_output_line(server, lines).await else {
            return false;
        };
        let Some(server) = server.upgrade() else {
            return false;
        };
        server.dispatch_line(&line).await;
        true
    }

    async fn dispatch_line(&self, line: &str) {
        match serde_json::from_str::<Value>(line) {
            Ok(message) => self.dispatch(message).await,
            Err(error) => tracing::warn!(%error, %line, "invalid app-server JSONL message"),
        }
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
        self.complete_response(tx, &message);
    }

    async fn fail_pending(&self, reason: &str) {
        for (_, response) in self.pending.lock().await.drain() {
            self.fail_response(response, reason);
        }
    }

    fn complete_response(&self, response: PendingResponse, message: &Value) {
        match response {
            PendingResponse::Awaited(tx) => {
                let _ = tx.send(protocol::awaited_result(message));
            }
            PendingResponse::Detached { thread_id } => {
                self.dispatch_detached_response(thread_id, message);
            }
        }
    }

    fn dispatch_detached_response(&self, thread_id: Value, message: &Value) {
        if let Some(error) = message.get("error") {
            self.dispatch_detached_error(thread_id, error);
        }
    }

    fn fail_response(&self, response: PendingResponse, reason: &str) {
        match response {
            PendingResponse::Awaited(tx) => {
                let _ = tx.send(Err(reason.to_owned()));
            }
            PendingResponse::Detached { thread_id } => {
                self.dispatch_detached_error(thread_id, &reason);
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

#[cfg(test)]
include!("app_server_tests.rs");
