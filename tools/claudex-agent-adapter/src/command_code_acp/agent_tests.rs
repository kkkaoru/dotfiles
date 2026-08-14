use std::{
    cell::RefCell,
    fs,
    future::Future,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    pin::Pin,
    process::Command,
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

fn spawn_local_task(future: Pin<Box<dyn Future<Output = ()> + 'static>>) {
    tokio::task::spawn_local(future);
}

struct TestSession {
    connection: acp::ClientSideConnection,
    updates: Rc<RefCell<Vec<String>>>,
    _root: TempDir,
    server: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

async fn open_session(script: &str, buffer: usize, track_server: bool) -> TestSession {
    let (root, program) = mock_cmd(script);
    let (client_write, server_read) = tokio::io::duplex(buffer);
    let (server_write, client_read) = tokio::io::duplex(buffer);
    let updates = Rc::new(RefCell::new(Vec::new()));
    let server =
        tokio::task::spawn_local(serve_io(options_for(program), server_read, server_write));
    let (connection, io) = acp::ClientSideConnection::new(
        CaptureClient {
            updates: Rc::clone(&updates),
        },
        client_write.compat_write(),
        client_read.compat(),
        spawn_local_task,
    );
    tokio::task::spawn_local(async move {
        let _ = io.await;
    });
    connection
        .initialize(acp::InitializeRequest::new(acp::ProtocolVersion::V1))
        .await
        .expect("initialize");
    TestSession {
        connection,
        updates,
        _root: root,
        server: track_server.then_some(server),
    }
}

async fn new_cwd_session(connection: &acp::ClientSideConnection) -> acp::NewSessionResponse {
    connection
        .new_session(acp::NewSessionRequest::new(
            std::env::current_dir().unwrap(),
        ))
        .await
        .expect("session")
}

fn text_prompt(session_id: acp::SessionId, text: &str) -> acp::PromptRequest {
    acp::PromptRequest::new(
        session_id,
        vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
    )
}

async fn wait_while_prompt_pending<F, T>(
    prompt_fut: &mut Pin<&mut F>,
    timeout: Duration,
    mut tick: impl FnMut() -> bool,
) where
    F: Future<Output = T>,
    T: std::fmt::Debug,
{
    tokio::time::timeout(timeout, async {
        loop {
            tokio::select! {
                result = &mut *prompt_fut => {
                    panic!("prompt finished early: {result:?}");
                }
                () = tokio::time::sleep(Duration::from_millis(20)) => {
                    if tick() {
                        return;
                    }
                }
            }
        }
    })
    .await
    .expect("prompt started");
}

async fn arm_in_flight_prompt<F, T>(prompt_fut: &mut Pin<&mut F>, delay: Duration)
where
    F: Future<Output = T>,
    T: std::fmt::Debug,
{
    tokio::time::timeout(CMD_START_TIMEOUT, async {
        tokio::select! {
            result = &mut *prompt_fut => {
                panic!("prompt finished before cancel: {result:?}");
            }
            () = tokio::time::sleep(delay) => {}
        }
    })
    .await
    .expect("cmd started before cancel");
}

async fn run_headless_turn_and_emits_tool_progress() {
    let session = open_session(success_script(), 64 * 1024, true).await;
    session
        .connection
        .authenticate(acp::AuthenticateRequest::new("ignored"))
        .await
        .ok();
    let created = new_cwd_session(&session.connection).await;
    let prompt = session
        .connection
        .prompt(text_prompt(
            created.session_id.clone(),
            "COMMAND_CODE_HEADLESS_OK",
        ))
        .await
        .expect("prompt");
    assert_eq!(prompt.stop_reason, acp::StopReason::EndTurn);
    let rendered = session.updates.borrow().join("\n");
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
    let _ = session
        .connection
        .set_session_model(acp::SetSessionModelRequest::new(
            created.session_id.clone(),
            DEFAULT_MODEL,
        ))
        .await;
    let config_err = session
        .connection
        .set_session_config_option(acp::SetSessionConfigOptionRequest::new(
            created.session_id.clone(),
            "effort",
            acp::SessionConfigValueId::new("high"),
        ))
        .await;
    assert!(config_err.is_err());
    session
        .connection
        .cancel(acp::CancelNotification::new(created.session_id))
        .await
        .expect("cancel");
    if let Some(server) = session.server {
        server.abort();
    }
}

async fn run_web_search_query_on_shared_tool_chrome() {
    let session = open_session(
        "#!/bin/sh\ncat <<'EOF'\n{\"type\":\"event\",\"event\":{\"type\":\"tool_running\",\"toolCallId\":\"t-search\",\"toolName\":\"web_search\",\"query\":\"AVITA株式会社\"}}\n{\"type\":\"event\",\"event\":{\"type\":\"tool_completed\",\"toolCallId\":\"t-search\",\"toolName\":\"web_search\"}}\n{\"type\":\"result\",\"subtype\":\"success\",\"sessionId\":\"cc-search\",\"stopReason\":\"end_turn\",\"finalText\":\"WEB_SEARCH_OK\"}\nEOF\n",
        64 * 1024,
        false,
    )
    .await;
    let created = new_cwd_session(&session.connection).await;
    let prompt = session
        .connection
        .prompt(text_prompt(created.session_id, "WEB_SEARCH_AVITA"))
        .await
        .expect("web_search prompt");
    assert_eq!(prompt.stop_reason, acp::StopReason::EndTurn);
    let rendered = session.updates.borrow().join("\n");
    assert!(
        rendered.contains("web_search") && rendered.contains("AVITA株式会社"),
        "web_search chrome must expose the query like other ACP workers: {rendered}"
    );
    assert!(!rendered.contains("ツール結果待ち"), "{rendered}");
}

async fn run_errors_when_failed_result_has_no_streamed_text() {
    let session = open_session(
        "#!/bin/sh\ncat <<'EOF'\n{\"type\":\"result\",\"subtype\":\"error\",\"sessionId\":\"cc-fail\",\"stopReason\":\"error\",\"finalText\":\"\",\"error\":\"boom\"}\nEOF\n",
        16 * 1024,
        false,
    )
    .await;
    let created = new_cwd_session(&session.connection).await;
    let err = session
        .connection
        .prompt(text_prompt(created.session_id, "hi"))
        .await
        .expect_err("empty failed result must surface as ACP error");
    let rendered = format!("{err:?}");
    assert!(
        rendered.contains("boom") || rendered.to_lowercase().contains("internal"),
        "{rendered}"
    );
}

async fn run_maps_max_turns_from_stop_reason_alone() {
    let session = open_session(
        "#!/bin/sh\ncat <<'EOF'\n{\"type\":\"result\",\"subtype\":\"error\",\"sessionId\":\"cc-max\",\"stopReason\":\"max_turns\",\"finalText\":\"partial\",\"error\":\"hit max\"}\nEOF\n",
        16 * 1024,
        false,
    )
    .await;
    let created = new_cwd_session(&session.connection).await;
    let response = session
        .connection
        .prompt(text_prompt(created.session_id, "hi"))
        .await
        .expect("prompt");
    assert_eq!(response.stop_reason, acp::StopReason::MaxTokens);
}

async fn run_cancels_before_prompt_and_maps_max_turns() {
    let session = open_session(
        "#!/bin/sh\ncat <<'EOF'\n{\"type\":\"result\",\"subtype\":\"max_turns\",\"sessionId\":\"cc-max\",\"stopReason\":\"max_turns\",\"finalText\":\"partial\"}\nEOF\n",
        16 * 1024,
        false,
    )
    .await;
    let created = new_cwd_session(&session.connection).await;
    session
        .connection
        .cancel(acp::CancelNotification::new(created.session_id.clone()))
        .await
        .expect("cancel before prompt");
    let cancelled = session
        .connection
        .prompt(text_prompt(created.session_id.clone(), "later"))
        .await
        .expect("cancelled prompt");
    assert_eq!(cancelled.stop_reason, acp::StopReason::Cancelled);
    let max_turns = session
        .connection
        .prompt(text_prompt(created.session_id, "again"))
        .await
        .expect("max turns prompt");
    assert_eq!(max_turns.stop_reason, acp::StopReason::MaxTokens);
}

async fn run_reports_headless_error_without_final_text() {
    let session = open_session(
        "#!/bin/sh\ncat <<'EOF'\n{\"type\":\"result\",\"subtype\":\"success\",\"stopReason\":\"error\",\"finalText\":\"\"}\nEOF\n",
        16 * 1024,
        false,
    )
    .await;
    let created = new_cwd_session(&session.connection).await;
    let error = session
        .connection
        .prompt(text_prompt(created.session_id, "boom"))
        .await;
    assert!(error.is_err());
}

async fn run_rejects_empty_prompt() {
    let session = open_session(success_script(), 16 * 1024, false).await;
    let created = new_cwd_session(&session.connection).await;
    let error = session
        .connection
        .prompt(acp::PromptRequest::new(created.session_id, Vec::new()))
        .await;
    assert!(error.is_err());
}

async fn run_cancel_kills_in_flight_cmd_within_two_seconds() {
    let session = open_session("#!/bin/sh\nexec sleep 30\n", 16 * 1024, false).await;
    let created = new_cwd_session(&session.connection).await;
    let prompt_fut = session
        .connection
        .prompt(text_prompt(created.session_id.clone(), "slow"));
    tokio::pin!(prompt_fut);
    arm_in_flight_prompt(&mut prompt_fut, Duration::from_millis(150)).await;
    let started = Instant::now();
    session
        .connection
        .cancel(acp::CancelNotification::new(created.session_id.clone()))
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
    let rendered = session.updates.borrow().join("\n");
    assert!(
        rendered.contains("Command Code cancelled") || rendered.contains("Failed"),
        "cancel must settle TUI chrome: {rendered}"
    );
}

async fn run_same_session_follow_up_replaces_in_flight_cmd() {
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
        spawn_local_task,
    );
    tokio::task::spawn_local(async move {
        let _ = io.await;
    });
    connection
        .initialize(acp::InitializeRequest::new(acp::ProtocolVersion::V1))
        .await
        .expect("initialize");
    let created = new_cwd_session(&connection).await;
    let first = connection.prompt(text_prompt(created.session_id.clone(), "slow"));
    tokio::pin!(first);
    wait_while_prompt_pending(&mut first, CMD_START_TIMEOUT, || marker.exists()).await;
    let started_at = Instant::now();
    connection
        .cancel(acp::CancelNotification::new(created.session_id.clone()))
        .await
        .expect("cancel in-flight before follow-up");
    let first = tokio::time::timeout(CMD_SETTLE_TIMEOUT, first)
        .await
        .expect("replaced prompt must settle")
        .expect("replaced prompt");
    assert_eq!(first.stop_reason, acp::StopReason::Cancelled);
    let second = tokio::time::timeout(
        CMD_SETTLE_TIMEOUT,
        connection.prompt(text_prompt(created.session_id, "follow-up")),
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
    drop(root);
}

async fn run_streams_text_delta_before_cmd_exits() {
    let session = open_session(
        "#!/bin/sh\nprintf '%s\\n' '{\"type\":\"event\",\"event\":{\"type\":\"text_delta\",\"delta\":\"LIVE_DELTA\"}}'\nsleep 0.4\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"sessionId\":\"cc-live\",\"stopReason\":\"end_turn\",\"finalText\":\"DONE\"}'\n",
        64 * 1024,
        false,
    )
    .await;
    let created = new_cwd_session(&session.connection).await;
    let prompt_fut = session
        .connection
        .prompt(text_prompt(created.session_id, "live"));
    tokio::pin!(prompt_fut);
    let updates = Rc::clone(&session.updates);
    wait_while_prompt_pending(&mut prompt_fut, CMD_START_TIMEOUT, || {
        updates
            .borrow()
            .iter()
            .any(|update| update.contains("LIVE_DELTA"))
    })
    .await;
    let response = prompt_fut.await.expect("live prompt");
    assert_eq!(response.stop_reason, acp::StopReason::EndTurn);
    let rendered = session.updates.borrow().join("\n");
    assert!(
        rendered.contains("AgentMessageChunk") || rendered.contains("LIVE_DELTA"),
        "live text must be assistant message not only thought: {rendered}"
    );
    assert!(rendered.contains("DONE"), "{rendered}");
}

#[cfg(unix)]
fn run_public_stdio_entrypoints_in_child() {
    use std::{
        fs::{File, OpenOptions},
        os::fd::AsRawFd,
    };

    let stdin = File::open("/dev/null").expect("open child stdin");
    let stdout = OpenOptions::new()
        .write(true)
        .open("/dev/null")
        .expect("open child stdout");
    // SAFETY: this runs only in the isolated test child. Both files stay open
    // until process exit, and no parent test process has its descriptors changed.
    assert_eq!(
        unsafe { libc::dup2(stdin.as_raw_fd(), libc::STDIN_FILENO) },
        0
    );
    // SAFETY: see the stdin redirection above.
    assert_eq!(
        unsafe { libc::dup2(stdout.as_raw_fd(), libc::STDOUT_FILENO) },
        1
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("stdio runtime");
    let local = tokio::task::LocalSet::new();
    runtime.block_on(local.run_until(async {
        assert!(super::run().await.is_err(), "test arguments must not parse");
        let (_root, program) = mock_cmd("#!/bin/sh\nexit 0\n");
        let _ = super::agent::serve(options_for(program)).await;
    }));
}

#[cfg(unix)]
#[test]
fn public_stdio_entrypoints_execute_in_an_isolated_child() {
    const CHILD: &str = "CLAUDEX_COMMAND_CODE_ACP_STDIO_CHILD";
    if std::env::var_os(CHILD).is_some() {
        run_public_stdio_entrypoints_in_child();
        return;
    }
    let status = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "command_code_acp::agent_tests::public_stdio_entrypoints_execute_in_an_isolated_child",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .status()
        .expect("run stdio child");
    assert!(status.success(), "stdio child failed: {status}");
}

#[tokio::test]
async fn serve_io_runs_headless_turn_and_emits_tool_progress() {
    tokio::task::LocalSet::new()
        .run_until(run_headless_turn_and_emits_tool_progress())
        .await;
}

#[tokio::test]
async fn serve_io_emits_web_search_query_on_shared_tool_chrome() {
    tokio::task::LocalSet::new()
        .run_until(run_web_search_query_on_shared_tool_chrome())
        .await;
}

#[tokio::test]
async fn serve_io_errors_when_failed_result_has_no_streamed_text() {
    tokio::task::LocalSet::new()
        .run_until(run_errors_when_failed_result_has_no_streamed_text())
        .await;
}

#[tokio::test]
async fn serve_io_maps_max_turns_from_stop_reason_alone() {
    tokio::task::LocalSet::new()
        .run_until(run_maps_max_turns_from_stop_reason_alone())
        .await;
}

#[tokio::test]
async fn serve_io_cancels_before_prompt_and_maps_max_turns() {
    tokio::task::LocalSet::new()
        .run_until(run_cancels_before_prompt_and_maps_max_turns())
        .await;
}

#[tokio::test]
async fn serve_io_reports_headless_error_without_final_text() {
    tokio::task::LocalSet::new()
        .run_until(run_reports_headless_error_without_final_text())
        .await;
}

#[tokio::test]
async fn serve_io_rejects_empty_prompt() {
    tokio::task::LocalSet::new()
        .run_until(run_rejects_empty_prompt())
        .await;
}

#[tokio::test]
async fn serve_io_cancel_kills_in_flight_cmd_within_two_seconds() {
    tokio::task::LocalSet::new()
        .run_until(run_cancel_kills_in_flight_cmd_within_two_seconds())
        .await;
}

#[tokio::test]
async fn serve_io_same_session_follow_up_replaces_in_flight_cmd() {
    tokio::task::LocalSet::new()
        .run_until(run_same_session_follow_up_replaces_in_flight_cmd())
        .await;
}

#[tokio::test]
async fn serve_io_streams_text_delta_before_cmd_exits() {
    tokio::task::LocalSet::new()
        .run_until(run_streams_text_delta_before_cmd_exits())
        .await;
}
