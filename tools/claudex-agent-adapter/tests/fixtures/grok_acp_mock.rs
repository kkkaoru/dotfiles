use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    fs::OpenOptions,
    io::Write as _,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

use agent_client_protocol::{self as acp, Client as _};
use serde_json::value::RawValue;
use tokio::{
    io::AsyncReadExt as _,
    net::UnixListener,
    sync::{Barrier, Notify, mpsc, oneshot},
};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

const TRACE_FILE: &str = "grok-acp-mock.jsonl";
const CLOSE_IO_FILE: &str = "grok-acp-close-io";
const SETUP_RELEASE_SOCKET: &str = "grok-acp-setup-release.sock";
const PARALLEL_RELEASE_FILE: &str = "grok-acp-parallel-release";
const CONFIGURED_PARALLEL_SESSIONS: usize = 7;
const PARALLEL_RELEASE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);
const THINKING_STREAM_MODE_SUFFIX: &str = "thinking-stream";
const THINKING_STREAM_MARKER: &str = "ACP_THINKING_MARKER";

struct MockAgent {
    operations: mpsc::UnboundedSender<ClientOperation>,
    trace: PathBuf,
    mode: String,
    next_session: Cell<u64>,
    concurrent_sessions: Cell<usize>,
    session_barrier: Option<Rc<Barrier>>,
    concurrent_prompts: Cell<usize>,
    both_prompts_started: Notify,
    cancellable_prompts: RefCell<HashMap<String, Rc<Notify>>>,
    cancelled_sessions: RefCell<HashSet<String>>,
    parallel_limit: usize,
    // Consumed on the first blocked set_model so later requests are not stuck.
    setup_release: RefCell<Option<UnixListener>>,
}

enum ClientOperation {
    Notify(acp::SessionNotification, oneshot::Sender<()>),
    Extension(acp::ExtNotification, oneshot::Sender<()>),
    Permission(
        acp::RequestPermissionRequest,
        oneshot::Sender<acp::Result<acp::RequestPermissionResponse>>,
    ),
}

async fn relay_client_operations(
    connection: acp::AgentSideConnection,
    mut requests: mpsc::UnboundedReceiver<ClientOperation>,
) {
    while let Some(request) = requests.recv().await {
        match request {
            ClientOperation::Notify(notification, sent) => {
                let _ = connection.session_notification(notification).await;
                let _ = sent.send(());
            }
            ClientOperation::Extension(notification, sent) => {
                let _ = connection.ext_notification(notification).await;
                let _ = sent.send(());
            }
            ClientOperation::Permission(request, response) => {
                let result = connection.request_permission(request).await;
                let _ = response.send(result);
            }
        }
    }
}

impl MockAgent {
    fn record(&self, event: &str, value: impl serde::Serialize) -> acp::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.trace)
            .map_err(|_| acp::Error::internal_error())?;
        serde_json::to_writer(&mut file, &serde_json::json!({ event: value }))
            .map_err(|_| acp::Error::internal_error())?;
        writeln!(file).map_err(|_| acp::Error::internal_error())
    }

    async fn notify(
        &self,
        session_id: acp::SessionId,
        update: acp::SessionUpdate,
    ) -> acp::Result<()> {
        let (sent, received) = oneshot::channel();
        self.operations
            .send(ClientOperation::Notify(
                acp::SessionNotification::new(session_id, update),
                sent,
            ))
            .map_err(|_| acp::Error::internal_error())?;
        received.await.map_err(|_| acp::Error::internal_error())
    }

    async fn notify_extension(&self, method: &str, params: serde_json::Value) -> acp::Result<()> {
        let raw =
            RawValue::from_string(params.to_string()).map_err(|_| acp::Error::internal_error())?;
        let (sent, received) = oneshot::channel();
        self.operations
            .send(ClientOperation::Extension(
                acp::ExtNotification::new(method, Arc::from(raw)),
                sent,
            ))
            .map_err(|_| acp::Error::internal_error())?;
        received.await.map_err(|_| acp::Error::internal_error())
    }

    async fn send_coverage_updates(&self, session_id: acp::SessionId) -> acp::Result<()> {
        self.notify(
            session_id.clone(),
            acp::SessionUpdate::ToolCall(
                acp::ToolCall::new("provider-read", "Read config")
                    .kind(acp::ToolKind::Read)
                    .status(acp::ToolCallStatus::InProgress)
                    .raw_input(serde_json::json!({"path":"config.toml"})),
            ),
        )
        .await?;
        for fields in [
            acp::ToolCallUpdateFields::new(),
            acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
            acp::ToolCallUpdateFields::new()
                .status(acp::ToolCallStatus::Pending)
                .title("Pending"),
            acp::ToolCallUpdateFields::new()
                .status(acp::ToolCallStatus::Completed)
                .title("Completed search"),
            acp::ToolCallUpdateFields::new()
                .status(acp::ToolCallStatus::Failed)
                .title("Failed search"),
        ] {
            self.notify(
                session_id.clone(),
                acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new("tool", fields)),
            )
            .await?;
        }
        for (method, params) in coverage_extensions(&session_id.0) {
            self.notify_extension(method, params).await?;
        }
        Ok(())
    }

    async fn send_read_empty(&self, session_id: acp::SessionId) -> acp::Result<()> {
        const CALL_ID: &str = "read-empty";
        self.notify(
            session_id.clone(),
            acp::SessionUpdate::ToolCall(
                acp::ToolCall::new(CALL_ID, "Read config")
                    .kind(acp::ToolKind::Read)
                    .status(acp::ToolCallStatus::InProgress)
                    .raw_input(serde_json::json!({"path":"config.toml"})),
            ),
        )
        .await?;
        self.notify(
            session_id,
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                CALL_ID,
                acp::ToolCallUpdateFields::new()
                    .status(acp::ToolCallStatus::Completed)
                    .title("Read config"),
            )),
        )
        .await
    }

    async fn wait_for_concurrent_prompt(&self, expected: usize) {
        let count = self.concurrent_prompts.get() + 1;
        self.concurrent_prompts.set(count);
        if count < expected {
            self.both_prompts_started.notified().await;
        } else {
            self.both_prompts_started.notify_waiters();
        }
        if expected <= 2 {
            return;
        }
        let release = self.trace.with_file_name(PARALLEL_RELEASE_FILE);
        while !release.exists() {
            tokio::time::sleep(PARALLEL_RELEASE_POLL_INTERVAL).await;
        }
    }

    async fn maybe_cancellable_prompt(
        &self,
        request: &acp::PromptRequest,
    ) -> acp::Result<Option<acp::PromptResponse>> {
        if !matches!(
            self.mode.as_str(),
            "cancellable-turns" | "ignored-cancellation" | "ignored-setup"
        ) || prompt_contains(request, "COMPLETE")
        {
            return Ok(None);
        }
        let session_id = request.session_id.0.to_string();
        self.record("prompt_submitted", request)?;
        if self.mode == "ignored-cancellation" {
            return std::future::pending::<acp::Result<Option<acp::PromptResponse>>>().await;
        }
        let cancelled = Rc::new(Notify::new());
        self.cancellable_prompts
            .borrow_mut()
            .insert(session_id.clone(), Rc::clone(&cancelled));
        let already_cancelled = self.cancelled_sessions.borrow_mut().remove(&session_id);
        if !already_cancelled {
            cancelled.notified().await;
            self.cancelled_sessions.borrow_mut().remove(&session_id);
        }
        self.cancellable_prompts.borrow_mut().remove(&session_id);
        Ok(Some(acp::PromptResponse::new(acp::StopReason::Cancelled)))
    }

    async fn complete_prompt_with_permission(
        &self,
        request: acp::PromptRequest,
    ) -> acp::Result<acp::PromptResponse> {
        let fields = if self.mode == "command-probe" {
            acp::ToolCallUpdateFields::new()
                .kind(acp::ToolKind::Execute)
                .title("Run harmless git and gh command probe")
                .raw_input(serde_json::json!({
                    "command":"command -v git && command -v gh"
                }))
        } else {
            acp::ToolCallUpdateFields::new().title("Mock tool")
        };
        let permission = acp::RequestPermissionRequest::new(
            request.session_id.clone(),
            acp::ToolCallUpdate::new("tool-call", fields),
            vec![
                acp::PermissionOption::new(
                    "allow-once",
                    "Allow once",
                    acp::PermissionOptionKind::AllowOnce,
                ),
                acp::PermissionOption::new(
                    "reject-once",
                    "Reject",
                    acp::PermissionOptionKind::RejectOnce,
                ),
            ],
        );
        let (permission_tx, permission_rx) = oneshot::channel();
        self.operations
            .send(ClientOperation::Permission(permission, permission_tx))
            .map_err(|_| acp::Error::internal_error())?;
        let permission_response = permission_rx
            .await
            .map_err(|_| acp::Error::internal_error())??;
        self.record("permission_response", &permission_response)?;
        if self.mode == "command-probe" {
            let result = permission_was_approved(&permission_response)
                .then(command_probe_result)
                .unwrap_or("ACP_COMMAND_PROBE_DENIED");
            self.record("command_probe", result)?;
            self.notify(
                request.session_id,
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(result.into())),
            )
            .await?;
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }
        for update in [
            acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
                acp::TextContent::new("ignored user"),
            ))),
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                acp::ContentBlock::Image(acp::ImageContent::new("data", "image/png")),
            )),
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
                acp::TextContent::new(""),
            ))),
        ] {
            self.notify(request.session_id.clone(), update).await?;
        }
        let output = if self.mode == "nested-boundary" {
            "GROK_ACP_CHILD_BOUNDARY_MARKER"
        } else {
            "GROK_ACP_STREAM_OK"
        };
        self.notify(
            request.session_id,
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(output.into())),
        )
        .await?;
        Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
    }
}

fn permission_was_approved(response: &acp::RequestPermissionResponse) -> bool {
    matches!(
        &response.outcome,
        acp::RequestPermissionOutcome::Selected(selected)
            if selected.option_id.0.as_ref() == "allow-once"
    )
}

fn command_probe_result() -> &'static str {
    let succeeded = std::process::Command::new("sh")
        .args([
            "-c",
            "command -v git >/dev/null && command -v gh >/dev/null && printf ACP_COMMAND_PROBE_OK >/dev/null",
        ])
        .status()
        .is_ok_and(|status| status.success());
    if succeeded {
        "ACP_COMMAND_PROBE_OK"
    } else {
        "ACP_COMMAND_PROBE_UNAVAILABLE"
    }
}

fn coverage_extensions(session_id: &str) -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("unrelated", serde_json::json!({})),
        ("_x.ai/session/update", serde_json::json!({})),
        (
            "_x.ai/session/update",
            serde_json::json!({ "sessionId": session_id }),
        ),
        (
            "_x.ai/session/update",
            serde_json::json!({"sessionId":session_id,"update":{}}),
        ),
        (
            "_x.ai/session/update",
            serde_json::json!({"sessionId":session_id,"update":{
            "sessionUpdate":"subagent_spawned"}}),
        ),
        (
            "_x.ai/session/update",
            serde_json::json!({"sessionId":session_id,"update":{
            "sessionUpdate":"subagent_spawned","description":"Research","model":"grok-4.6",
            "reasoning_effort":"medium"}}),
        ),
        (
            "_x.ai/session/update",
            serde_json::json!({"sessionId":session_id,"update":{
            "sessionUpdate":"subagent_finished"}}),
        ),
        (
            "_x.ai/session/update",
            serde_json::json!({"sessionId":session_id,"update":{
            "sessionUpdate":"subagent_finished","status":"completed","duration_ms":1250}}),
        ),
        (
            "_x.ai/session/update",
            serde_json::json!({"sessionId":session_id,"update":{
            "sessionUpdate":"retry_state"}}),
        ),
        (
            "_x.ai/session/update",
            serde_json::json!({"sessionId":session_id,"update":{
            "sessionUpdate":"retry_state","attempt":2,"max_retries":4}}),
        ),
        (
            "_x.ai/session/update",
            serde_json::json!({"sessionId":session_id,"update":{
            "sessionUpdate":"turn_completed"}}),
        ),
        (
            "_x.ai/session/update",
            serde_json::json!({"sessionId":session_id,"update":{
            "sessionUpdate":"turn_completed","usage":{}}}),
        ),
    ]
}

#[async_trait::async_trait(?Send)]
impl acp::Agent for MockAgent {
    async fn initialize(
        &self,
        request: acp::InitializeRequest,
    ) -> acp::Result<acp::InitializeResponse> {
        self.record("initialize", request)?;
        if self.mode == "ignored-initialize" {
            return std::future::pending::<acp::Result<acp::InitializeResponse>>().await;
        }
        if self.mode == "fail-initialize" {
            return Err(acp::Error::internal_error());
        }
        if self.mode == "bad-version" {
            return Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V0));
        }
        if self.mode == "no-auth" {
            return Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1));
        }
        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1)
            .auth_methods(vec![acp::AuthMethod::Agent(acp::AuthMethodAgent::new(
                "cached_token",
                "Cached token",
            ))])
            .meta(
                serde_json::json!({"defaultAuthMethodId":"cached_token"})
                    .as_object()
                    .cloned(),
            ))
    }

    async fn authenticate(
        &self,
        request: acp::AuthenticateRequest,
    ) -> acp::Result<acp::AuthenticateResponse> {
        self.record("authenticate", request)?;
        if self.mode == "fail-auth" {
            return Err(acp::Error::internal_error());
        }
        Ok(acp::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        request: acp::NewSessionRequest,
    ) -> acp::Result<acp::NewSessionResponse> {
        self.record("new_session", &request)?;
        if let Some(barrier) = &self.session_barrier {
            let count = self.concurrent_sessions.get() + 1;
            self.concurrent_sessions.set(count);
            if count <= self.parallel_limit {
                barrier.wait().await;
                let release = self.trace.with_file_name(PARALLEL_RELEASE_FILE);
                while !release.exists() {
                    tokio::time::sleep(PARALLEL_RELEASE_POLL_INTERVAL).await;
                }
            }
        }
        if self.mode == "exit-once-session" {
            let marker = self.trace.with_file_name("grok-acp-exited-once");
            if !marker.exists() {
                std::fs::write(marker, b"exited").map_err(|_| acp::Error::internal_error())?;
                std::process::exit(9);
            }
        }
        if self.mode == "fail-session" {
            return Err(acp::Error::internal_error());
        }
        if self.mode == "fail-mcp-new" && !request.mcp_servers.is_empty() {
            return Err(acp::Error::internal_error());
        }
        if self.mode == "fail-parallel-session" && self.next_session.get() > 0 {
            return Err(acp::Error::internal_error());
        }
        if self.mode == "dropped-parallel-session" && self.next_session.get() > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let next = self.next_session.get() + 1;
        self.next_session.set(next);
        Ok(acp::NewSessionResponse::new(format!("grok-session-{next}")))
    }

    async fn load_session(
        &self,
        request: acp::LoadSessionRequest,
    ) -> acp::Result<acp::LoadSessionResponse> {
        self.record("load_session", request)?;
        Ok(acp::LoadSessionResponse::new())
    }

    async fn prompt(&self, request: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
        self.record("prompt", &request)?;
        if self.mode == "no-response" {
            // A real quota stall can keep the ACP process and its stdio alive
            // while never producing an update or prompt response.
            return std::future::pending::<acp::Result<acp::PromptResponse>>().await;
        }
        if self.mode == "no-response-first" {
            let marker = self.trace.with_file_name("grok-acp-no-response-first");
            if !marker.exists() {
                std::fs::write(marker, b"stalled").map_err(|_| acp::Error::internal_error())?;
                return std::future::pending::<acp::Result<acp::PromptResponse>>().await;
            }
        }
        if matches!(
            self.mode.as_str(),
            "fail-parallel-session" | "dropped-parallel-session"
        ) {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        if self.mode == "fail-prompt" {
            return Err(acp::Error::internal_error());
        }
        if self.mode == "fail-prompt-once" {
            let marker = self.trace.with_file_name("grok-acp-prompt-failed-once");
            if !marker.exists() {
                std::fs::write(marker, b"failed").map_err(|_| acp::Error::internal_error())?;
                return Err(acp::Error::internal_error());
            }
        }
        if self.mode == "coverage-updates" {
            self.send_coverage_updates(request.session_id.clone())
                .await?;
        }
        if self.mode == "read-empty" {
            self.send_read_empty(request.session_id.clone()).await?;
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }
        if self.mode == "xai-spawn" {
            self.notify_extension(
                "_x.ai/session/update",
                serde_json::json!({
                    "sessionId": request.session_id.0,
                    "update": {
                        "sessionUpdate": "subagent_spawned",
                        "description": "Research",
                        "model": "grok-4.6",
                        "reasoning_effort": "medium"
                    }
                }),
            )
            .await?;
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }
        if self.mode == "concurrent-turns" {
            self.wait_for_concurrent_prompt(2).await;
        }
        if self.mode == "concurrent-turns-seven" {
            self.wait_for_concurrent_prompt(self.parallel_limit).await;
        }
        if let Some(response) = self.maybe_cancellable_prompt(&request).await? {
            return Ok(response);
        }
        if self.mode.ends_with(THINKING_STREAM_MODE_SUFFIX) {
            self.notify(
                request.session_id.clone(),
                acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
                    THINKING_STREAM_MARKER.into(),
                )),
            )
            .await?;
        }
        self.complete_prompt_with_permission(request).await
    }

    async fn cancel(&self, request: acp::CancelNotification) -> acp::Result<()> {
        self.record("cancel", &request)?;
        let session_id = request.session_id.0.to_string();
        self.cancelled_sessions
            .borrow_mut()
            .insert(session_id.clone());
        if self.mode == "ignored-cancellation" {
            return Ok(());
        }
        if let Some(cancelled) = self.cancellable_prompts.borrow().get(&session_id).cloned() {
            cancelled.notify_one();
        }
        Ok(())
    }

    async fn set_session_model(
        &self,
        request: acp::SetSessionModelRequest,
    ) -> acp::Result<acp::SetSessionModelResponse> {
        self.record("set_model", &request)?;
        if self.mode == "fail-effort" {
            return Err(acp::Error::invalid_params());
        }
        if self.mode == "ignored-setup" {
            self.record("set_model_blocked", &request)?;
            return std::future::pending::<acp::Result<acp::SetSessionModelResponse>>().await;
        }
        if self.mode == "blocked-effort" {
            let listener = self.setup_release.borrow_mut().take();
            if let Some(listener) = listener {
                let (mut release, _) =
                    tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                        .await
                        .map_err(|_| acp::Error::internal_error())?
                        .map_err(|_| acp::Error::internal_error())?;
                self.record("set_model_blocked", &request)?;
                let mut signal = [0];
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    release.read_exact(&mut signal),
                )
                .await
                .map_err(|_| acp::Error::internal_error())?
                .map_err(|_| acp::Error::internal_error())?;
                self.record("set_model_settled", &request)?;
            }
        }
        Ok(acp::SetSessionModelResponse::default())
    }

    async fn set_session_config_option(
        &self,
        request: acp::SetSessionConfigOptionRequest,
    ) -> acp::Result<acp::SetSessionConfigOptionResponse> {
        self.record("set_effort", &request)?;
        match self.mode.as_str() {
            "effort-config" => Ok(acp::SetSessionConfigOptionResponse::new(vec![])),
            "reject-effort" => Err(acp::Error::invalid_params()),
            _ => Err(acp::Error::method_not_found()),
        }
    }
}

async fn run_agent(
    agent: MockAgent,
    requests: mpsc::UnboundedReceiver<ClientOperation>,
) -> acp::Result<()> {
    let close_io_marker =
        (agent.mode == "close-io").then(|| agent.trace.with_file_name(CLOSE_IO_FILE));
    let (connection, io) = acp::AgentSideConnection::new(
        agent,
        tokio::io::stdout().compat_write(),
        tokio::io::stdin().compat(),
        |future| {
            tokio::task::spawn_local(future);
        },
    );
    tokio::task::spawn_local(relay_client_operations(connection, requests));
    if let Some(marker) = close_io_marker {
        let io = tokio::task::spawn_local(io);
        while !marker.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        io.abort();
        let _ = io.await;
        // Tokio's stdio wrappers borrow the process descriptors, so dropping
        // their I/O future alone does not close the provider pipe endpoints.
        // Close them explicitly while keeping this fixture process alive so
        // the adapter observes I/O closure before it reaps the child.
        #[cfg(unix)]
        // SAFETY: this fixture is intentionally terminating its own ACP stdio.
        unsafe {
            libc::close(libc::STDIN_FILENO);
            libc::close(libc::STDOUT_FILENO);
        }
        return std::future::pending::<acp::Result<()>>().await;
    }
    io.await
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> acp::Result<()> {
    let cwd = std::env::current_dir().map_err(|_| acp::Error::internal_error())?;
    let trace = cwd.join(TRACE_FILE);
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    validate_native_grok_arguments(&args)?;
    let mode = args.get(1).cloned().unwrap_or_default();
    let parallel_limit = parallel_limit_from_args(&args);
    let session_barrier =
        (mode == "concurrent-sessions-at-limit").then(|| Rc::new(Barrier::new(parallel_limit)));
    let setup_release = if mode == "blocked-effort" {
        Some(UnixListener::bind(SETUP_RELEASE_SOCKET).map_err(|_| acp::Error::internal_error())?)
    } else {
        None
    };
    let (operations, requests) = mpsc::unbounded_channel();
    let agent = MockAgent {
        operations,
        trace,
        mode,
        next_session: Cell::new(0),
        concurrent_sessions: Cell::new(0),
        session_barrier,
        concurrent_prompts: Cell::new(0),
        both_prompts_started: Notify::new(),
        cancellable_prompts: RefCell::new(HashMap::new()),
        cancelled_sessions: RefCell::new(HashSet::new()),
        parallel_limit,
        setup_release: RefCell::new(setup_release),
    };
    agent.record("arguments", &args)?;
    agent.record("claudex_grok_acp", std::env::var("CLAUDEX_GROK_ACP").ok())?;
    let local = tokio::task::LocalSet::new();
    local.run_until(run_agent(agent, requests)).await
}

fn validate_native_grok_arguments(args: &[String]) -> acp::Result<()> {
    if std::env::var("CLAUDEX_GROK_ACP").as_deref() != Ok("1") {
        return Ok(());
    }
    let valid_prefix = matches!(
        args,
        [model_flag, _, effort_flag, effort, ..]
            if model_flag == "--model"
                && effort_flag == "--reasoning-effort"
                && matches!(effort.as_str(), "low" | "medium" | "high")
    );
    let valid_tail = matches!(
        args.get(4..),
        Some([agent, approve, no_leader, stdio])
            if agent == "agent"
                && approve == "--always-approve"
                && no_leader == "--no-leader"
                && stdio == "stdio"
    ) || matches!(
        args.get(4..),
        Some([agent, approve, no_leader, plugin_flag, plugin_dir, stdio])
            if agent == "agent"
                && approve == "--always-approve"
                && no_leader == "--no-leader"
                && plugin_flag == "--plugin-dir"
                && !plugin_dir.is_empty()
                && stdio == "stdio"
    );
    let valid = valid_prefix && valid_tail;
    if valid {
        Ok(())
    } else {
        Err(acp::Error::invalid_params())
    }
}

fn parallel_limit_from_args(args: &[String]) -> usize {
    args.windows(2)
        .find(|pair| pair[0] == "--parallel-limit")
        .and_then(|pair| pair[1].parse().ok())
        .filter(|limit| *limit > 0)
        .unwrap_or(CONFIGURED_PARALLEL_SESSIONS)
}

fn prompt_contains(request: &acp::PromptRequest, expected: &str) -> bool {
    serde_json::to_value(request)
        .ok()
        .and_then(|value| {
            value
                .pointer("/prompt/0/text")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|text| text.contains(expected))
}
