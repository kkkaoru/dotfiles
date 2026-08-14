use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
};

use agent_client_protocol as acp;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::AbortHandle;

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
}

#[path = "agent_prompt.rs"]
mod prompt_handlers;

#[path = "agent_acp.rs"]
mod acp_impl;

#[path = "agent_serve.rs"]
mod serve;
pub use serve::serve;
#[cfg(test)]
pub(super) use serve::serve_io;

#[cfg(test)]
#[path = "agent/local_tests.rs"]
mod local_tests;
