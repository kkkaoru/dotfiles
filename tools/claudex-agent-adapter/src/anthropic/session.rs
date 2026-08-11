use std::sync::Arc;

use anyhow::Result;

use super::{
    ActiveTurn, Bridge, MessagesRequest, SelectedSession, Session,
    content::{ToolResult, collect_turn_tool_results, request_signature},
};

mod continuation;
mod helpers;
mod preempt;
#[cfg(test)]
pub(super) mod reservation;
#[cfg(not(test))]
mod reservation;
mod results;
mod select;
mod session_turn;
mod tools;
use helpers::{first_session_owning_results, should_preempt_for_context_limit};
// Child modules and session_tests reach these through the parent namespace.
#[allow(unused_imports)]
use super::content::transcript_owns_tool_results;
#[allow(unused_imports)]
use helpers::{
    candidate_length, is_better_length, is_idempotent_task_lifecycle_error, touch_session,
    validate_tool_result_ownership,
};
pub(in crate::anthropic) use session_turn::is_context_window_exceeded;
pub(in crate::anthropic) use tools::is_main_session_only_tool;
#[cfg(test)]
pub(super) use tools::{
    codex_tool_name, dynamic_tool, thread_start_params, thread_start_params_for_mode,
    tool_configuration, tool_configuration_for_mode,
};

impl Bridge {
    pub(super) async fn prepare_turn(
        &self,
        request: &MessagesRequest,
        input_tokens: u64,
        effort: Option<String>,
    ) -> Result<ActiveTurn> {
        self.remove_failed_model_sessions(&self.request_model(request))
            .await;
        let model = self.request_model(request);
        let advisor_model = self
            .advisor_model_override
            .clone()
            .or_else(|| self.claude_setting("advisorModel"));
        let collaborator_model = request
            .claudex_collaborator_model
            .clone()
            .or_else(|| self.collaborator_model_override.clone())
            .or_else(|| self.claude_collaborator_model());
        let signature = request_signature(
            request,
            advisor_model.as_deref(),
            collaborator_model.as_deref(),
        )?;
        let signature =
            self.intern_signature(format!("{}\0{signature}", self.request_model(request)));
        let tool_results = collect_turn_tool_results(&request.messages);
        let mut selected = self
            .select_session(
                request,
                signature,
                advisor_model.as_deref(),
                collaborator_model.as_deref(),
                &tool_results,
            )
            .await?;
        let context_limit = self.app.max_context_tokens_for_model(&model);
        if should_preempt_for_context_limit(input_tokens, context_limit, !tool_results.is_empty()) {
            let limit = context_limit.expect("preemption requires a context limit");
            tracing::warn!(
                limit,
                input_tokens,
                model = %model,
                "claudex: preemptively starting fresh thread before context limit"
            );
            selected = self
                .start_new_session(
                    request,
                    &selected,
                    advisor_model.as_deref(),
                    collaborator_model.as_deref(),
                )
                .await?;
        }
        self.start_selected_turn(session_turn::StartSelectedTurn {
            request,
            input_tokens,
            effort,
            selected,
            tool_results,
            advisor_model: advisor_model.as_deref(),
            collaborator_model: collaborator_model.as_deref(),
            allow_context_retry: true,
        })
        .await
    }

    async fn remove_failed_model_sessions(&self, model: &str) {
        if self.app.model_is_alive(model) {
            return;
        }
        self.sessions
            .lock()
            .await
            .retain(|session| session.model != model);
    }

    pub(super) async fn remove_session(&self, removed: &Arc<Session>) {
        self.sessions
            .lock()
            .await
            .retain(|session| !Arc::ptr_eq(session, removed));
    }

    /// Move a backgrounded turn out of active-session matching without
    /// discarding its pending tool-result ownership. A late Claude Code result
    /// must still be routed to the provider thread that emitted the tool call,
    /// while a new main request must be free to create/reuse another session.
    pub(super) async fn detach_session(&self, detached: &Arc<Session>) {
        self.sessions
            .lock()
            .await
            .retain(|session| !Arc::ptr_eq(session, detached));
        let mut detached_sessions = self.detached_sessions.lock().await;
        if !detached_sessions
            .iter()
            .any(|session| Arc::ptr_eq(session, detached))
        {
            detached_sessions.push(Arc::clone(detached));
        }
    }

    pub(super) async fn finish_detached_session(&self, finished: &Arc<Session>) {
        // Move completed detached session back to active list for idle reuse.
        // SAFETY: pending tools must be empty; unresolved tool_use blocks reattach.
        let pending = finished.pending_tools.lock().await;
        if !pending.is_empty() {
            tracing::warn!(session_id = %finished.thread_id, "cannot reattach with pending tools");
            return;
        }
        drop(pending);

        let mut detached = self.detached_sessions.lock().await;
        let Some(index) = detached.iter().position(|s| Arc::ptr_eq(s, finished)) else {
            return;
        };
        let session = detached.remove(index);
        drop(detached);
        self.reattach_finished_session(session).await;
    }

    async fn reattach_finished_session(&self, session: Arc<Session>) {
        let mut active = self.sessions.lock().await;
        if !active.iter().any(|s| Arc::ptr_eq(s, &session)) {
            active.push(session);
        }
    }

    pub(super) async fn is_detached_session(&self, candidate: &Arc<Session>) -> bool {
        self.detached_sessions
            .lock()
            .await
            .iter()
            .any(|session| Arc::ptr_eq(session, candidate))
    }

    pub(super) async fn find_result_session(&self, results: &[ToolResult]) -> Option<Arc<Session>> {
        let mut sessions = self.sessions.lock().await.clone();
        sessions.extend(self.detached_sessions.lock().await.iter().cloned());
        first_session_owning_results(sessions, results).await
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
