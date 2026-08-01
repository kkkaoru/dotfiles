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
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::excessive_nesting)]
mod tests {
    use std::{process::Stdio, sync::Weak};

    use tokio::{io::AsyncBufReadExt as _, process::Command};

    use super::*;
    use crate::app_server::AppServer;

    #[tokio::test]
    async fn reads_a_line_and_handles_clean_output_eof() {
        let mut child = Command::new("sh")
            .args(["-c", "printf 'ready\\n'"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn line fixture");
        let stdout = child.stdout.take().expect("stdout pipe");
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        assert_eq!(
            next_output_line(&Weak::new(), &mut lines).await.as_deref(),
            Some("ready")
        );
        assert!(next_output_line(&Weak::new(), &mut lines).await.is_none());
        child.wait().await.expect("wait for line fixture");
    }

    #[tokio::test]
    async fn reports_a_closed_stdout_read_error() {
        struct ErrorReader;

        impl tokio::io::AsyncRead for ErrorReader {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
                _buffer: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Err(std::io::Error::other("synthetic read failure")))
            }
        }

        let mut lines = tokio::io::BufReader::new(ErrorReader).lines();
        assert!(
            next_output_line(&Weak::<AppServer>::new(), &mut lines)
                .await
                .is_none()
        );
    }

    #[test]
    fn resolves_result_and_error_json_rpc_frames() {
        assert_eq!(
            awaited_result(&serde_json::json!({"result": {"ok": true}})),
            Ok(serde_json::json!({"ok": true}))
        );
        assert_eq!(
            awaited_result(&serde_json::json!({"error": {"code": -1}})),
            Err(r#"{"code":-1}"#.to_owned())
        );
        assert_eq!(awaited_result(&serde_json::json!({})), Ok(Value::Null));
    }
}
