use std::{
    cell::RefCell,
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

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

// Ceiling only. llvm-cov instrumentation regularly exceeds the old 2s windows.
const CMD_START_TIMEOUT: Duration = Duration::from_secs(20);
const CMD_SETTLE_TIMEOUT: Duration = Duration::from_secs(20);

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
            assert!(
                rendered.contains("InProgress") || rendered.contains("read_file"),
                "native tool chrome missing: {rendered}"
            );
            assert!(
                rendered.contains("read_file") || rendered.contains("README.md"),
                "native tool chrome missing: {rendered}"
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
async fn serve_io_emits_web_search_query_on_shared_tool_chrome() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (_root, program) = mock_cmd(
                "#!/bin/sh\ncat <<'EOF'\n{\"type\":\"event\",\"event\":{\"type\":\"tool_running\",\"toolCallId\":\"t-search\",\"toolName\":\"web_search\",\"query\":\"AVITA株式会社\"}}\n{\"type\":\"event\",\"event\":{\"type\":\"tool_completed\",\"toolCallId\":\"t-search\",\"toolName\":\"web_search\"}}\n{\"type\":\"result\",\"subtype\":\"success\",\"sessionId\":\"cc-search\",\"stopReason\":\"end_turn\",\"finalText\":\"WEB_SEARCH_OK\"}\nEOF\n",
            );
            let (client_write, server_read) = tokio::io::duplex(64 * 1024);
            let (server_write, client_read) = tokio::io::duplex(64 * 1024);
            let updates = Rc::new(RefCell::new(Vec::new()));
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
            let session = connection
                .new_session(acp::NewSessionRequest::new(
                    std::env::current_dir().unwrap(),
                ))
                .await
                .expect("session");
            let prompt = connection
                .prompt(acp::PromptRequest::new(
                    session.session_id,
                    vec![acp::ContentBlock::Text(acp::TextContent::new(
                        "WEB_SEARCH_AVITA",
                    ))],
                ))
                .await
                .expect("web_search prompt");
            assert_eq!(prompt.stop_reason, acp::StopReason::EndTurn);
            let rendered = updates.borrow().join("\n");
            assert!(
                rendered.contains("web_search") && rendered.contains("AVITA株式会社"),
                "web_search chrome must expose the query like other ACP workers: {rendered}"
            );
            assert!(!rendered.contains("ツール結果待ち"), "{rendered}");
        })
        .await;
}

#[tokio::test]
async fn serve_io_maps_max_turns_from_stop_reason_alone() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (_root, program) = mock_cmd(
                "#!/bin/sh\ncat <<'EOF'\n{\"type\":\"result\",\"subtype\":\"error\",\"sessionId\":\"cc-max\",\"stopReason\":\"max_turns\",\"finalText\":\"partial\",\"error\":\"hit max\"}\nEOF\n",
            );
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
            let response = connection
                .prompt(acp::PromptRequest::new(
                    session.session_id.clone(),
                    vec![acp::ContentBlock::Text(acp::TextContent::new("hi"))],
                ))
                .await
                .expect("prompt");
            assert_eq!(response.stop_reason, acp::StopReason::MaxTokens);
        })
        .await;
}

#[tokio::test]
async fn serve_io_cancels_before_prompt_and_maps_max_turns() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (_root, program) = mock_cmd(
                "#!/bin/sh\ncat <<'EOF'\n{\"type\":\"result\",\"subtype\":\"max_turns\",\"sessionId\":\"cc-max\",\"stopReason\":\"max_turns\",\"finalText\":\"partial\"}\nEOF\n",
            );
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
            connection
                .cancel(acp::CancelNotification::new(session.session_id.clone()))
                .await
                .expect("cancel before prompt");
            let cancelled = connection
                .prompt(acp::PromptRequest::new(
                    session.session_id.clone(),
                    vec![acp::ContentBlock::Text(acp::TextContent::new("later"))],
                ))
                .await
                .expect("cancelled prompt");
            assert_eq!(cancelled.stop_reason, acp::StopReason::Cancelled);

            let max_turns = connection
                .prompt(acp::PromptRequest::new(
                    session.session_id,
                    vec![acp::ContentBlock::Text(acp::TextContent::new("again"))],
                ))
                .await
                .expect("max turns prompt");
            assert_eq!(max_turns.stop_reason, acp::StopReason::MaxTokens);
        })
        .await;
}

#[tokio::test]
async fn serve_io_reports_headless_error_without_final_text() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (_root, program) = mock_cmd(
                "#!/bin/sh\ncat <<'EOF'\n{\"type\":\"result\",\"subtype\":\"success\",\"stopReason\":\"error\",\"finalText\":\"\"}\nEOF\n",
            );
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
                .prompt(acp::PromptRequest::new(
                    session.session_id,
                    vec![acp::ContentBlock::Text(acp::TextContent::new("boom"))],
                ))
                .await;
            assert!(error.is_err());
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

#[tokio::test]
async fn serve_io_cancel_kills_in_flight_cmd_within_two_seconds() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (_root, program) = mock_cmd("#!/bin/sh\nexec sleep 30\n");
            let (client_write, server_read) = tokio::io::duplex(16 * 1024);
            let (server_write, client_read) = tokio::io::duplex(16 * 1024);
            let updates = Rc::new(RefCell::new(Vec::new()));
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
            let session = connection
                .new_session(acp::NewSessionRequest::new(
                    std::env::current_dir().unwrap(),
                ))
                .await
                .expect("session");
            let prompt_fut = connection.prompt(acp::PromptRequest::new(
                session.session_id.clone(),
                vec![acp::ContentBlock::Text(acp::TextContent::new("slow"))],
            ));
            tokio::pin!(prompt_fut);
            tokio::time::timeout(CMD_START_TIMEOUT, async {
                tokio::select! {
                    result = &mut prompt_fut => {
                        panic!("prompt finished before cancel: {result:?}");
                    }
                    () = tokio::time::sleep(Duration::from_millis(150)) => {}
                }
            })
            .await
            .expect("cmd started before cancel");
            let started = Instant::now();
            connection
                .cancel(acp::CancelNotification::new(session.session_id.clone()))
                .await
                .expect("cancel in flight");
            let response = tokio::time::timeout(CMD_SETTLE_TIMEOUT, prompt_fut)
                .await
                .expect("cancel should settle")
                .expect("cancelled prompt");
            assert_eq!(response.stop_reason, acp::StopReason::Cancelled);
            assert!(
                started.elapsed() < CMD_SETTLE_TIMEOUT,
                "cancel took {:?}",
                started.elapsed()
            );
            let rendered = updates.borrow().join("\n");
            assert!(
                rendered.contains("Command Code cancelled") || rendered.contains("Failed"),
                "cancel must settle TUI chrome: {rendered}"
            );
        })
        .await;
}

#[tokio::test]
async fn serve_io_same_session_follow_up_replaces_in_flight_cmd() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (root, program) = mock_cmd("#!/bin/sh\nexec sleep 30\n");
            let marker = root.path().join("started");
            std::fs::write(
                &program,
                format!(
                    "#!/bin/sh\nif [ -f '{0}' ]; then\nprintf '%s\\n' '{{\"type\":\"result\",\"subtype\":\"success\",\"sessionId\":\"cc-2\",\"stopReason\":\"end_turn\",\"finalText\":\"FOLLOW_UP_OK\"}}'\nexit 0\nfi\n: > '{0}'\nexec sleep 30\n",
                    marker.display()
                ),
            )
            .expect("rewrite mock");
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
            let first = connection.prompt(acp::PromptRequest::new(
                session.session_id.clone(),
                vec![acp::ContentBlock::Text(acp::TextContent::new("slow"))],
            ));
            tokio::pin!(first);
            tokio::time::timeout(CMD_START_TIMEOUT, async {
                loop {
                    tokio::select! {
                        result = &mut first => {
                            panic!("first prompt finished before follow-up: {result:?}");
                        }
                        () = tokio::time::sleep(Duration::from_millis(20)) => {
                            if marker.exists() {
                                return;
                            }
                        }
                    }
                }
            })
            .await
            .expect("first cmd started");
            let started_at = Instant::now();
            connection
                .cancel(acp::CancelNotification::new(session.session_id.clone()))
                .await
                .expect("cancel in-flight before follow-up");
            let first = tokio::time::timeout(CMD_SETTLE_TIMEOUT, first)
                .await
                .expect("replaced prompt must settle")
                .expect("replaced prompt");
            assert_eq!(first.stop_reason, acp::StopReason::Cancelled);
            let second = tokio::time::timeout(
                CMD_SETTLE_TIMEOUT,
                connection.prompt(acp::PromptRequest::new(
                    session.session_id,
                    vec![acp::ContentBlock::Text(acp::TextContent::new("follow-up"))],
                )),
            )
            .await
            .expect("follow-up must start immediately after cancel")
            .expect("follow-up prompt");
            assert_eq!(second.stop_reason, acp::StopReason::EndTurn);
            assert!(
                started_at.elapsed() < CMD_SETTLE_TIMEOUT,
                "follow-up waited {:?}",
                started_at.elapsed()
            );
        })
        .await;
}

#[tokio::test]
async fn serve_io_streams_text_delta_before_cmd_exits() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let (_root, program) = mock_cmd(
                "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"event\",\"event\":{\"type\":\"text_delta\",\"delta\":\"LIVE_DELTA\"}}'\nsleep 0.4\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"sessionId\":\"cc-live\",\"stopReason\":\"end_turn\",\"finalText\":\"DONE\"}'\n",
            );
            let (client_write, server_read) = tokio::io::duplex(64 * 1024);
            let (server_write, client_read) = tokio::io::duplex(64 * 1024);
            let updates = Rc::new(RefCell::new(Vec::new()));
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
            let session = connection
                .new_session(acp::NewSessionRequest::new(
                    std::env::current_dir().unwrap(),
                ))
                .await
                .expect("session");
            let prompt_fut = connection.prompt(acp::PromptRequest::new(
                session.session_id.clone(),
                vec![acp::ContentBlock::Text(acp::TextContent::new("live"))],
            ));
            tokio::pin!(prompt_fut);
            tokio::time::timeout(Duration::from_secs(8), async {
                loop {
                    tokio::select! {
                        result = &mut prompt_fut => {
                            panic!("prompt finished before live delta: {result:?}");
                        }
                        _ = tokio::time::sleep(Duration::from_millis(20)) => {
                            if updates.borrow().iter().any(|update| update.contains("LIVE_DELTA"))
                            {
                                return;
                            }
                        }
                    }
                }
            })
            .await
            .expect("text_delta must reach ACP before cmd exits");
            let response = prompt_fut.await.expect("live prompt");
            assert_eq!(response.stop_reason, acp::StopReason::EndTurn);
            let rendered = updates.borrow().join("\n");
            assert!(
                rendered.contains("AgentMessageChunk") || rendered.contains("LIVE_DELTA"),
                "live text must be assistant message not only thought: {rendered}"
            );
            assert!(rendered.contains("DONE"), "{rendered}");
        })
        .await;
}
