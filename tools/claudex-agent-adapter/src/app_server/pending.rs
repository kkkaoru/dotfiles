use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::sync::oneshot;

pub(super) struct PendingRequest {
    pub(super) id: u64,
    pub(super) response: oneshot::Receiver<Result<Value, String>>,
}

pub(super) enum PendingResponse {
    Awaited(oneshot::Sender<Result<Value, String>>),
    Detached { thread_id: Value },
}

pub(super) async fn await_response(rx: oneshot::Receiver<Result<Value, String>>) -> Result<Value> {
    match rx.await.context("app-server response channel closed")? {
        Ok(value) => Ok(value),
        Err(message) => bail!(message),
    }
}
