use std::{cell::Cell, fs::OpenOptions, io::Write as _, path::PathBuf, sync::Arc};

use agent_client_protocol::{self as acp, Client as _};
use serde_json::value::RawValue;
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

const TRACE_FILE: &str = "grok-acp-mock.jsonl";

struct MockAgent {
    operations: mpsc::UnboundedSender<ClientOperation>,
    trace: PathBuf,
    mode: String,
    next_session: Cell<u64>,
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
        serde_json::to_writer(&mut file, &serde_json::json!({event:value}))
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
}

fn coverage_extensions(session_id: &str) -> Vec<(&'static str, serde_json::Value)> {
    vec![
        ("unrelated", serde_json::json!({})),
        ("_x.ai/session/update", serde_json::json!({})),
        (
            "_x.ai/session/update",
            serde_json::json!({"sessionId":session_id}),
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
            "sessionUpdate":"subagent_spawned","description":"Research","model":"grok-4.5",
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
        self.record("new_session", request)?;
        if self.mode == "fail-session" {
            return Err(acp::Error::internal_error());
        }
        let next = self.next_session.get() + 1;
        self.next_session.set(next);
        Ok(acp::NewSessionResponse::new(format!("grok-session-{next}")))
    }

    async fn prompt(&self, request: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
        self.record("prompt", &request)?;
        if self.mode == "fail-prompt" {
            return Err(acp::Error::internal_error());
        }
        if self.mode == "coverage-updates" {
            self.send_coverage_updates(request.session_id.clone())
                .await?;
        }
        let permission = acp::RequestPermissionRequest::new(
            request.session_id.clone(),
            acp::ToolCallUpdate::new(
                "tool-call",
                acp::ToolCallUpdateFields::new().title("Mock tool"),
            ),
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
        self.record("permission_response", permission_response)?;
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
        self.notify(
            request.session_id,
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                "GROK_ACP_STREAM_OK".into(),
            )),
        )
        .await?;
        Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
    }

    async fn cancel(&self, request: acp::CancelNotification) -> acp::Result<()> {
        self.record("cancel", request)
    }

    async fn set_session_model(
        &self,
        request: acp::SetSessionModelRequest,
    ) -> acp::Result<acp::SetSessionModelResponse> {
        self.record("set_model", request)?;
        if self.mode == "fail-effort" {
            return Err(acp::Error::invalid_params());
        }
        Ok(acp::SetSessionModelResponse::default())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> acp::Result<()> {
    let trace = std::env::current_dir()
        .map_err(|_| acp::Error::internal_error())?
        .join(TRACE_FILE);
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let mode = args.get(1).cloned().unwrap_or_default();
    let (operations, requests) = mpsc::unbounded_channel();
    let agent = MockAgent {
        operations,
        trace,
        mode,
        next_session: Cell::new(0),
    };
    agent.record("arguments", &args)?;
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let (connection, io) = acp::AgentSideConnection::new(
                agent,
                tokio::io::stdout().compat_write(),
                tokio::io::stdin().compat(),
                |future| {
                    tokio::task::spawn_local(future);
                },
            );
            tokio::task::spawn_local(relay_client_operations(connection, requests));
            io.await
        })
        .await
}
