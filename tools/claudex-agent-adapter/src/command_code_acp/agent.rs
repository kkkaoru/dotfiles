use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
};

use agent_client_protocol::{self as acp, Client as _};
use anyhow::Result;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::AbortHandle;
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};
use uuid::Uuid;

use super::{
    coalesce::message_text_from_progress,
    events::progress_to_updates,
    options::Options,
    prompt::prompt_text,
};

mod emit;
use emit::{emit_cancelled, emit_result};

enum ClientOperation {
    Notify(acp::SessionNotification, oneshot::Sender<()>),
}

struct HeadlessAgent {
    options: Options,
    operations: mpsc::UnboundedSender<ClientOperation>,
    next_session: Cell<u64>,
    session_cwds: RefCell<HashMap<String, PathBuf>>,
    cancelled: RefCell<HashMap<String, bool>>,
    running: RefCell<HashMap<String, AbortHandle>>,
    prompt_lock: Mutex<()>,
}

impl HeadlessAgent {
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

    fn session_key(session_id: &acp::SessionId) -> String {
        session_id.0.to_string()
    }

    fn take_cancelled(&self, session_id: &str) -> bool {
        self.cancelled
            .borrow_mut()
            .remove(session_id)
            .unwrap_or(false)
    }

    fn track_running(&self, session_id: &str, handle: AbortHandle) {
        let abort_now = {
            self.running
                .borrow_mut()
                .insert(session_id.to_owned(), handle.clone());
            self.cancelled
                .borrow()
                .get(session_id)
                .copied()
                .unwrap_or(false)
        };
        if abort_now {
            handle.abort();
        }
    }

    fn untrack_running(&self, session_id: &str) {
        self.running.borrow_mut().remove(session_id);
    }

    fn abort_running(&self, session_id: &str) {
        if let Some(handle) = self.running.borrow().get(session_id) {
            handle.abort();
        }
    }

    async fn run_prompt_turn(
        &self,
        session_id: &acp::SessionId,
        prompt: &str,
        resume: Option<&str>,
    ) -> acp::Result<Option<super::process::TurnOutcome>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let operations = self.operations.clone();
        let notify_session = session_id.clone();
        let emit = tokio::task::spawn_local(emit_progress_events(rx, operations, notify_session));
        let spec = self.options.spec.clone();
        let prompt = prompt.to_owned();
        let resume = resume.map(str::to_owned);
        let key = Self::session_key(session_id);
        let cwd = self.session_cwds.borrow().get(&key).cloned();
        let run = tokio::task::spawn_local(async move {
            super::process::run_turn_emitting(
                &spec,
                &prompt,
                resume.as_deref(),
                Some(tx),
                cwd.as_deref(),
            )
            .await
        });
        self.track_running(&key, run.abort_handle());
        let joined = run.await;
        self.untrack_running(&key);
        let _ = emit.await;
        match joined {
            Ok(Ok(outcome)) => Ok(Some(outcome)),
            Ok(Err(error)) => Err(acp::Error::internal_error().data(error.to_string())),
            Err(join) if join.is_cancelled() => Ok(None),
            Err(join) => Err(acp::Error::internal_error().data(join.to_string())),
        }
    }

    fn open_session(&self, cwd: PathBuf) -> String {
        let next = self.next_session.get() + 1;
        self.next_session.set(next);
        let session_id = format!("command-code-{}", Uuid::new_v4());
        self.session_cwds
            .borrow_mut()
            .insert(session_id.clone(), cwd);
        session_id
    }

    async fn handle_prompt(&self, request: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
        let session_key = Self::session_key(&request.session_id);
        // Same-session TUI follow-ups must replace in-flight cmd -p instead of
        // stacking behind it. Cross-session prompts still serialize on the lock.
        self.abort_running(&session_key);
        let _prompt = self.prompt_lock.lock().await;
        if self.take_cancelled(&session_key) {
            return Ok(acp::PromptResponse::new(acp::StopReason::Cancelled));
        }
        let prompt = prompt_text(&request);
        if prompt.trim().is_empty() {
            return Err(acp::Error::invalid_params());
        }
        // SubAgent turns are one-shot. Resuming cmd's last project session is
        // what produced Muse Spark's "Ready to continue — I see ~N modified
        // files" greeting instead of the delegated task.
        let Some(outcome) = self
            .run_prompt_turn(&request.session_id, &prompt, None)
            .await?
        else {
            self.take_cancelled(&session_key);
            return emit_cancelled(self, request.session_id).await;
        };
        if self.take_cancelled(&session_key) {
            return emit_cancelled(self, request.session_id).await;
        }
        let streamed = message_text_from_progress(&outcome.progress);
        emit_result(self, request.session_id, &outcome.result, &streamed).await
    }

    fn handle_cancel(&self, session_id: &acp::SessionId) {
        let key = Self::session_key(session_id);
        self.cancelled.borrow_mut().insert(key.clone(), true);
        self.abort_running(&key);
    }
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
        }
    }
}

fn forward_progress_updates(
    operations: &mpsc::UnboundedSender<ClientOperation>,
    session_id: &acp::SessionId,
    event: &super::events::ProgressEvent,
) -> bool {
    for update in progress_to_updates(event) {
        // Fire-and-forget: waiting for ACP ack serializes live ▶/thinking
        // behind prompt() completion on the client.
        let (sent, _received) = oneshot::channel();
        if operations
            .send(ClientOperation::Notify(
                acp::SessionNotification::new(session_id.clone(), update),
                sent,
            ))
            .is_err()
        {
            return false;
        }
    }
    true
}

async fn emit_progress_events(
    mut rx: mpsc::UnboundedReceiver<super::events::ProgressEvent>,
    operations: mpsc::UnboundedSender<ClientOperation>,
    notify_session: acp::SessionId,
) {
    while keep_emitting_progress(&mut rx, &operations, &notify_session).await {}
}

async fn keep_emitting_progress(
    rx: &mut mpsc::UnboundedReceiver<super::events::ProgressEvent>,
    operations: &mpsc::UnboundedSender<ClientOperation>,
    notify_session: &acp::SessionId,
) -> bool {
    let Some(event) = rx.recv().await else {
        return false;
    };
    forward_progress_updates(operations, notify_session, &event)
}

// Nightly branch instrumentation emits an invalid mapping for async-trait's
// generated Agent shim (same llvm-cov getInstantiationGroups crash as Grok's
// ACP client). Fixture tests still cover the delegated prompt/cancel paths.
#[cfg_attr(coverage_nightly, coverage(off))]
#[async_trait::async_trait(?Send)]
impl acp::Agent for HeadlessAgent {
    async fn initialize(
        &self,
        _request: acp::InitializeRequest,
    ) -> acp::Result<acp::InitializeResponse> {
        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1))
    }

    async fn authenticate(
        &self,
        _request: acp::AuthenticateRequest,
    ) -> acp::Result<acp::AuthenticateResponse> {
        Ok(acp::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        request: acp::NewSessionRequest,
    ) -> acp::Result<acp::NewSessionResponse> {
        Ok(acp::NewSessionResponse::new(self.open_session(request.cwd)))
    }

    async fn prompt(&self, request: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
        self.handle_prompt(request).await
    }

    async fn cancel(&self, request: acp::CancelNotification) -> acp::Result<()> {
        self.handle_cancel(&request.session_id);
        Ok(())
    }

    async fn set_session_model(
        &self,
        _request: acp::SetSessionModelRequest,
    ) -> acp::Result<acp::SetSessionModelResponse> {
        Ok(acp::SetSessionModelResponse::default())
    }

    async fn set_session_config_option(
        &self,
        _request: acp::SetSessionConfigOptionRequest,
    ) -> acp::Result<acp::SetSessionConfigOptionResponse> {
        Err(acp::Error::method_not_found())
    }
}

pub async fn serve(options: Options) -> Result<()> {
    serve_io(options, tokio::io::stdin(), tokio::io::stdout()).await
}

pub(super) async fn serve_io<R, W>(options: Options, stdin: R, stdout: W) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin + 'static,
    W: tokio::io::AsyncWrite + Unpin + 'static,
{
    let (operations, requests) = mpsc::unbounded_channel();
    let agent = HeadlessAgent {
        options,
        operations,
        next_session: Cell::new(0),
        session_cwds: RefCell::new(HashMap::new()),
        cancelled: RefCell::new(HashMap::new()),
        running: RefCell::new(HashMap::new()),
        prompt_lock: Mutex::new(()),
    };
    let (connection, io) =
        acp::AgentSideConnection::new(agent, stdout.compat_write(), stdin.compat(), spawn_local);
    tokio::task::spawn_local(relay_client_operations(connection, requests));
    io.await.map_err(|error| anyhow::anyhow!("{error}"))
}

fn spawn_local(future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'static>>) {
    tokio::task::spawn_local(future);
}
