use std::{
    sync::{Weak, atomic::Ordering},
    time::Duration,
};

use anyhow::{Result, bail};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::oneshot,
};

use super::{
    AppServer, REQUEST_TIMEOUT, ThreadEvents,
    pending::{PendingRequest, PendingResponse, await_response},
    protocol,
};

impl AppServer {
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

    pub(super) async fn request_with_timeout(
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

    pub(super) async fn begin_request(&self, method: &str, params: Value) -> Result<PendingRequest> {
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

    pub(super) async fn write(&self, value: &Value) -> Result<()> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&line).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub(super) async fn read_loop(server: Weak<Self>, stdout: tokio::process::ChildStdout) {
        let mut lines = BufReader::new(stdout).lines();
        while Self::dispatch_next_line(&server, &mut lines).await {}
    }

    pub(super) async fn dispatch_next_line(
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

    pub(super) async fn dispatch_line(&self, line: &str) {
        match serde_json::from_str::<Value>(line) {
            Ok(message) => self.dispatch(message).await,
            Err(error) => tracing::warn!(%error, %line, "invalid app-server JSONL message"),
        }
    }

    pub(super) async fn dispatch(&self, message: Value) {
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

    pub(super) async fn fail_pending(&self, reason: &str) {
        for (_, response) in self.pending.lock().await.drain() {
            self.fail_response(response, reason);
        }
    }

    pub(super) fn complete_response(&self, response: PendingResponse, message: &Value) {
        match response {
            PendingResponse::Awaited(tx) => {
                let _ = tx.send(protocol::awaited_result(message));
            }
            PendingResponse::Detached { thread_id } => {
                self.dispatch_detached_response(thread_id, message);
            }
        }
    }

    pub(super) fn dispatch_detached_response(&self, thread_id: Value, message: &Value) {
        if let Some(error) = message.get("error") {
            self.dispatch_detached_error(thread_id, error);
        }
    }

    pub(super) fn fail_response(&self, response: PendingResponse, reason: &str) {
        match response {
            PendingResponse::Awaited(tx) => {
                let _ = tx.send(Err(reason.to_owned()));
            }
            PendingResponse::Detached { thread_id } => {
                self.dispatch_detached_error(thread_id, &reason);
            }
        }
    }

    pub(super) fn dispatch_detached_error(&self, thread_id: Value, error: &dyn std::fmt::Display) {
        self.event_dispatcher.dispatch(json!({
            "method":"error",
            "params":{
                "threadId":thread_id,
                "willRetry":false,
                "error":{"message":format!("turn/start failed: {error}")}}
        }));
    }
}
