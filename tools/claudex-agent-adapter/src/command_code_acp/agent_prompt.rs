use std::path::PathBuf;

use agent_client_protocol as acp;
use uuid::Uuid;

use super::{HeadlessAgent, emit_cancelled, emit_result, message_text_from_progress, prompt_text};

impl HeadlessAgent {
    pub(super) fn open_session(&self, cwd: PathBuf) -> String {
        let next = self.next_session.get() + 1;
        self.next_session.set(next);
        let session_id = format!("command-code-{}", Uuid::new_v4());
        self.session_cwds
            .borrow_mut()
            .insert(session_id.clone(), cwd);
        session_id
    }

    pub(super) async fn handle_prompt(
        &self,
        request: acp::PromptRequest,
    ) -> acp::Result<acp::PromptResponse> {
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

    pub(super) fn handle_cancel(&self, session_id: &acp::SessionId) {
        let key = Self::session_key(session_id);
        self.cancelled.borrow_mut().insert(key.clone(), true);
        self.abort_running(&key);
    }
}
