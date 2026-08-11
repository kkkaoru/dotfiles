#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::{process::Stdio, sync::Weak};

    use tokio::{io::AsyncBufReadExt as _, process::Command};

    use super::*;
    use crate::app_server::AppServer;

    struct ErrorReader(&'static str);

    impl tokio::io::AsyncRead for ErrorReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buffer: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::Error::other(self.0)))
        }
    }

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
        let mut lines = tokio::io::BufReader::new(ErrorReader("synthetic read failure")).lines();
        assert!(
            next_output_line(&Weak::<AppServer>::new(), &mut lines)
                .await
                .is_none()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stops_a_live_app_server_when_stdout_ends() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Arc;

        let root = tempfile::tempdir().expect("protocol stop fixture");
        let source = root.path().join("source");
        std::fs::create_dir(&source).expect("source home");
        std::fs::write(source.join("auth.json"), "{}").expect("auth");
        let program = root.path().join("protocol-eof");
        std::fs::write(
            &program,
            "#!/bin/sh\nread initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nwhile read line; do :; done\n",
        )
        .expect("write fixture");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("executable fixture");
        let server = AppServer::spawn_with_program(
            "model",
            &program,
            &source,
            &root.path().join("protocol-eof-home"),
        )
        .await
        .expect("start protocol fixture");
        let weak = Arc::downgrade(&server);
        let mut pipe = Command::new("sh")
            .args(["-c", "printf 'bye\\n'"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn eof pipe");
        let stdout = pipe.stdout.take().expect("stdout");
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        assert_eq!(
            next_output_line(&weak, &mut lines).await.as_deref(),
            Some("bye")
        );
        assert!(next_output_line(&weak, &mut lines).await.is_none());
        assert!(!server.is_alive());
        let _ = pipe.wait().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stops_a_live_app_server_on_stdout_read_error() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Arc;

        let root = tempfile::tempdir().expect("protocol error fixture");
        let source = root.path().join("source");
        std::fs::create_dir(&source).expect("source home");
        std::fs::write(source.join("auth.json"), "{}").expect("auth");
        let program = root.path().join("protocol-error");
        std::fs::write(
            &program,
            "#!/bin/sh\nread initialize\nprintf '%s\\n' '{\"id\":1,\"result\":{}}'\nread initialized\nwhile read line; do :; done\n",
        )
        .expect("write fixture");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("executable fixture");
        let server = AppServer::spawn_with_program(
            "model",
            &program,
            &source,
            &root.path().join("protocol-error-home"),
        )
        .await
        .expect("start protocol fixture");
        let weak = Arc::downgrade(&server);
        let mut lines =
            tokio::io::BufReader::new(ErrorReader("synthetic live read failure")).lines();
        assert!(next_output_line(&weak, &mut lines).await.is_none());
        assert!(!server.is_alive());
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
