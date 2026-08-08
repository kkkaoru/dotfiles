use std::{cell::RefCell, fs, os::unix::fs::PermissionsExt, path::PathBuf, rc::Rc};

use agent_client_protocol::{self as acp, Agent as _};
use tempfile::TempDir;
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

use super::{
    agent::serve_io,
    launch::{DEFAULT_MODEL, LaunchSpec},
    options::Options,
};

struct CaptureClient {
    updates: Rc<RefCell<Vec<String>>>,
}

#[async_trait::async_trait(?Send)]
impl acp::Client for CaptureClient {
    async fn request_permission(
        &self,
        request: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        let option = request
            .options
            .first()
            .map(|option| option.option_id.clone());
        Ok(acp::RequestPermissionResponse::new(option.map_or(
            acp::RequestPermissionOutcome::Cancelled,
            |option_id| {
                acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(
                    option_id,
                ))
            },
        )))
    }

    async fn session_notification(
        &self,
        notification: acp::SessionNotification,
    ) -> acp::Result<()> {
        self.updates
            .borrow_mut()
            .push(format!("{:?}", notification.update));
        Ok(())
    }
}

fn mock_cmd(body: &str) -> (TempDir, PathBuf) {
    let root = TempDir::new().expect("mock cmd");
    let path = root.path().join("cmd");
    fs::write(&path, body).expect("write mock");
    let mut permissions = fs::metadata(&path).expect("stat mock").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod mock");
    (root, path)
}

fn success_script() -> &'static str {
    "#!/bin/sh\ncat <<'EOF'\n{\"type\":\"event\",\"event\":{\"type\":\"tool_running\",\"toolCallId\":\"t1\",\"toolName\":\"read_file\",\"description\":\"README.md\"}}\n{\"type\":\"event\",\"event\":{\"type\":\"tool_completed\",\"toolCallId\":\"t1\",\"toolName\":\"read_file\"}}\n{\"type\":\"result\",\"subtype\":\"success\",\"sessionId\":\"cc-1\",\"stopReason\":\"end_turn\",\"finalText\":\"COMMAND_CODE_HEADLESS_OK\"}\nEOF\n"
}

fn options_for(program: PathBuf) -> Options {
    Options {
        spec: LaunchSpec {
            program,
            model: DEFAULT_MODEL.to_owned(),
            effort: Some("high".to_owned()),
            max_turns: 8,
            yolo: true,
            trust: true,
            skip_onboarding: true,
        },
    }
}

#[tokio::test]
async fn serve_io_runs_headless_turn_and_emits_tool_progress() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (_root, program) = mock_cmd(success_script());
            let (client_write, server_read) = tokio::io::duplex(64 * 1024);
            let (server_write, client_read) = tokio::io::duplex(64 * 1024);
            let updates = Rc::new(RefCell::new(Vec::new()));
            let server =
                tokio::task::spawn_local(serve_io(options_for(program), server_read, server_write));
            let (connection, io) = acp::ClientSideConnection::new(
                CaptureClient {
                    updates: Rc::clone(&updates),
                },
                client_write.compat_write(),
                client_read.compat(),
                |future| {
                    tokio::task::spawn_local(future);
                },
            );
            tokio::task::spawn_local(async move {
                let _ = io.await;
            });
            connection
                .initialize(acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                .await
                .expect("initialize");
            connection
                .authenticate(acp::AuthenticateRequest::new("ignored"))
                .await
                .ok();
            let session = connection
                .new_session(acp::NewSessionRequest::new(
                    std::env::current_dir().unwrap(),
                ))
                .await
                .expect("new session");
            let prompt = connection
                .prompt(acp::PromptRequest::new(
                    session.session_id.clone(),
                    vec![acp::ContentBlock::Text(acp::TextContent::new(
                        "COMMAND_CODE_HEADLESS_OK",
                    ))],
                ))
                .await
                .expect("prompt");
            assert_eq!(prompt.stop_reason, acp::StopReason::EndTurn);
            let rendered = updates.borrow().join("\n");
            assert!(
                rendered.contains("read_file") || rendered.contains("Command Code headless"),
                "{rendered}"
            );
            let _ = connection
                .set_session_model(acp::SetSessionModelRequest::new(
                    session.session_id.clone(),
                    DEFAULT_MODEL,
                ))
                .await;
            let config_err = connection
                .set_session_config_option(acp::SetSessionConfigOptionRequest::new(
                    session.session_id.clone(),
                    "effort",
                    acp::SessionConfigValueId::new("high"),
                ))
                .await;
            assert!(config_err.is_err());
            connection
                .cancel(acp::CancelNotification::new(session.session_id))
                .await
                .expect("cancel");
            server.abort();
        })
        .await;
}

#[tokio::test]
async fn serve_io_rejects_empty_prompt() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (_root, program) = mock_cmd(success_script());
            let (client_write, server_read) = tokio::io::duplex(16 * 1024);
            let (server_write, client_read) = tokio::io::duplex(16 * 1024);
            tokio::task::spawn_local(serve_io(options_for(program), server_read, server_write));
            let (connection, io) = acp::ClientSideConnection::new(
                CaptureClient {
                    updates: Rc::new(RefCell::new(Vec::new())),
                },
                client_write.compat_write(),
                client_read.compat(),
                |future| {
                    tokio::task::spawn_local(future);
                },
            );
            tokio::task::spawn_local(async move {
                let _ = io.await;
            });
            connection
                .initialize(acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                .await
                .expect("initialize");
            let session = connection
                .new_session(acp::NewSessionRequest::new(
                    std::env::current_dir().unwrap(),
                ))
                .await
                .expect("session");
            let error = connection
                .prompt(acp::PromptRequest::new(session.session_id, Vec::new()))
                .await;
            assert!(error.is_err());
        })
        .await;
}
