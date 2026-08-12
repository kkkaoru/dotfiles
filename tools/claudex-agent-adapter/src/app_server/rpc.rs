use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::{
    AppServer, REQUEST_TIMEOUT, ThreadEvents,
    pending::{PendingRequest, PendingResponse, await_response},
};

#[path = "rpc_dispatch.rs"]
mod dispatch;

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
        let started = Instant::now();
        let request = match tokio::time::timeout(timeout, self.begin_request(method, params)).await
        {
            Ok(request) => request?,
            Err(_) => {
                bail!("app-server request `{method}` timed out after {timeout:?}");
            }
        };
        let remaining = timeout.saturating_sub(started.elapsed());
        match tokio::time::timeout(remaining, await_response(request.response)).await {
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
        let mut line =
            serde_json::to_vec(&json!({ "id": id, "method": method, "params": params }))?;
        line.push(b'\n');
        let reservation = self.writer.reserve().await?;
        self.pending
            .lock()
            .await
            .insert(id, PendingResponse::Detached { thread_id });
        // `send` is synchronous after reserve, so cancellation cannot occur
        // between publishing pending state and queueing its frame.
        if let Err(error) = await_write(reservation.send(line)).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        Ok(())
    }

    pub(super) async fn begin_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<PendingRequest> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut line =
            serde_json::to_vec(&json!({ "id": id, "method": method, "params": params }))?;
        line.push(b'\n');
        let reservation = self.writer.reserve().await?;
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .await
            .insert(id, PendingResponse::Awaited(tx));
        if let Err(error) = await_write(reservation.send(line)).await {
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
        let reservation = self.writer.reserve().await?;
        await_write(reservation.send(line)).await
    }
}

async fn await_write(completion: tokio::sync::oneshot::Receiver<Result<(), String>>) -> Result<()> {
    match completion.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => bail!(error),
        Err(_) => bail!("app-server writer stopped before flushing its frame"),
    }
}
