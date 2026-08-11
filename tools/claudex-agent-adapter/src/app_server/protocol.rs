use std::sync::Weak;

use serde_json::Value;
use tokio::io::{AsyncRead, BufReader, Lines};

use super::{AppServer, lifecycle};

pub(super) async fn next_output_line<R>(
    server: &Weak<AppServer>,
    lines: &mut Lines<BufReader<R>>,
) -> Option<String>
where
    R: AsyncRead + Unpin,
{
    match lines.next_line().await {
        Ok(Some(line)) => Some(line),
        Ok(None) => {
            lifecycle::stop_if_alive(server, "codex app-server exited or closed its output").await;
            None
        }
        Err(error) => {
            tracing::error!(%error, "failed to read codex app-server output");
            lifecycle::stop_if_alive(
                server,
                &format!("failed to read codex app-server output: {error}"),
            )
            .await;
            None
        }
    }
}

pub(super) fn awaited_result(message: &Value) -> Result<Value, String> {
    message.get("error").map_or_else(
        || Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        |error| Err(error.to_string()),
    )
}

#[cfg(test)]
include!("protocol_tests.rs");

