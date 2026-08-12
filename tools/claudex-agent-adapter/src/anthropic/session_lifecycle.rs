use std::sync::Arc;

use super::{Bridge, Session, ToolResult, first_session_owning_results};

impl Bridge {
    pub(super) async fn remove_failed_model_sessions(&self, model: &str) {
        self.sessions.lock().await.retain(|session| {
            session.model != model || self.app_for_session(session).model_is_alive(model)
        });
    }

    pub(in crate::anthropic) async fn remove_session(&self, removed: &Arc<Session>) {
        self.sessions
            .lock()
            .await
            .retain(|session| !Arc::ptr_eq(session, removed));
    }

    /// Move a backgrounded turn out of active-session matching without
    /// discarding its pending tool-result ownership. A late Claude Code result
    /// must still be routed to the provider thread that emitted the tool call,
    /// while a new main request must be free to create/reuse another session.
    pub(in crate::anthropic) async fn detach_session(&self, detached: &Arc<Session>) {
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

    pub(in crate::anthropic) async fn finish_detached_session(&self, finished: &Arc<Session>) {
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

    pub(in crate::anthropic) async fn is_detached_session(&self, candidate: &Arc<Session>) -> bool {
        self.detached_sessions
            .lock()
            .await
            .iter()
            .any(|session| Arc::ptr_eq(session, candidate))
    }

    pub(in crate::anthropic) async fn find_result_session(
        &self,
        results: &[ToolResult],
    ) -> Option<Arc<Session>> {
        let mut sessions = self.sessions.lock().await.clone();
        sessions.extend(self.detached_sessions.lock().await.iter().cloned());
        first_session_owning_results(sessions, results).await
    }
}
