use std::sync::Weak;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

use super::super::{AppServer, pending::PendingResponse, protocol};

impl AppServer {
    pub(in crate::app_server) async fn read_loop(server: Weak<Self>, stdout: tokio::process::ChildStdout) {
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

    pub(in crate::app_server) async fn fail_pending(&self, reason: &str) {
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
