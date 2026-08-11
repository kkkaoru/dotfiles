use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
};

use agent_client_protocol as acp;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::AbortHandle;
use uuid::Uuid;

use super::{coalesce::message_text_from_progress, options::Options, prompt::prompt_text};

mod emit;
mod progress;
use emit::{emit_cancelled, emit_result};
use progress::emit_progress_events;

pub(super) enum ClientOperation {
    Notify(acp::SessionNotification, oneshot::Sender<()>),
}

pub(super) struct HeadlessAgent {
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

#[path = "agent_acp.rs"]
mod acp_impl;

#[path = "agent_serve.rs"]
mod serve;
pub use serve::serve;
#[cfg(test)]
pub(super) use serve::serve_io;

