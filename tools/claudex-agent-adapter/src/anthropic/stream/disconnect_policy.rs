use std::{collections::HashSet, sync::Arc};

use super::{
    helpers::{
        drain_disconnected_turn_with_warning, reject_disconnected_tool_with_warning,
        request_id_keys, take_pending_disconnected_tools,
    },
    warn_cancel_failure, warn_disconnect_failure,
};
use crate::{
    agent_backend::TurnCancellation,
    anthropic::{Bridge, Session, stream::StreamTurn},
};

impl Bridge {
    pub(super) async fn disconnect_stream_with_policy(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
        abort_visible_tool_provider: bool,
    ) -> StreamTurn {
        // Cancel before unregistering so a racing outer follow-up can still
        // discover this session, preempt the gate, and reuse the provider thread.
        match self
            .app_for_session(session)
            .cancel_turn(&session.thread_id)
            .await
        {
            Ok(TurnCancellation::Settled) => {
                let _ = self.reject_pending_disconnected_tools(session).await;
                self.pause_or_remove_disconnected_session(session).await;
            }
            Ok(TurnCancellation::Unsupported) => {
                self.handle_unsupported_disconnect(session, events, abort_visible_tool_provider)
                    .await;
            }
            Err(error) => {
                warn_cancel_failure(&error, &session.thread_id);
                self.detach_non_cancellable_turn(session, events).await;
                self.remove_session(session).await;
            }
        }
        StreamTurn::Disconnected
    }

    async fn pause_or_remove_disconnected_session(&self, session: &Arc<Session>) {
        if session.claude_session_id.is_none() {
            self.remove_session(session).await;
            return;
        }
        // Claude Code TaskStop / SSE close pauses the SubAgent.
        // Keep the Pi/provider thread so SendMessage({to}) can continue
        // the same session instead of spawning another. Agent({resume}) was removed.
        if let Ok(mut activity) = session.last_activity.lock() {
            *activity = std::time::Instant::now();
        }
    }

    pub(super) async fn handle_unsupported_disconnect(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
        abort_visible_tool_provider: bool,
    ) {
        if session.pending_tools.lock().await.is_empty() {
            // No tool call has reached Claude Code yet. Keep consuming the
            // non-cancellable turn so a delayed call receives a rejection.
            self.detach_non_cancellable_turn(session, events).await;
            self.remove_session(session).await;
            return;
        }
        if !abort_visible_tool_provider {
            // A native Agent handoff already returned control to Claude Code.
            // Reject the pending call and drain only this turn so a shared
            // provider remains available to unrelated sessions.
            self.detach_non_cancellable_turn(session, events).await;
            self.remove_session(session).await;
            return;
        }
        // A client-visible tool call can no longer receive a result. Abort and
        // reap the provider instead of leaving hidden work attached to it.
        self.remove_session(session).await;
        self.discard_pending_disconnected_tools(session).await;
        self.abort_disconnected_provider(session).await;
    }

    pub(super) async fn abort_disconnected_provider(&self, session: &Session) {
        if let Err(error) = self
            .app_for_session(session)
            .abort_turn_provider(&session.thread_id)
            .await
        {
            warn_disconnect_failure(
                &error,
                &session.thread_id,
                "failed to abort non-cancellable disconnected provider",
            );
        }
    }

    pub(super) async fn detach_non_cancellable_turn(
        &self,
        session: &Arc<Session>,
        events: Arc<crate::app_server::ThreadEvents>,
    ) {
        let rejected_request_ids = self.reject_pending_disconnected_tools(session).await;
        self.spawn_disconnected_turn_drain(session, events, rejected_request_ids);
    }

    pub(super) fn spawn_disconnected_turn_drain(
        &self,
        session: &Session,
        events: Arc<crate::app_server::ThreadEvents>,
        rejected_request_ids: HashSet<String>,
    ) {
        let app = self.app_for_session(session);
        tokio::spawn(drain_disconnected_turn_with_warning(
            app,
            session.model.clone(),
            events,
            rejected_request_ids,
        ));
    }

    pub(super) async fn reject_pending_disconnected_tools(
        &self,
        session: &Session,
    ) -> HashSet<String> {
        let pending = take_pending_disconnected_tools(session).await;
        self.agent_efforts
            .remove_tool_results(pending.iter().map(|(tool_use_id, _)| tool_use_id.as_str()));
        let rejected_request_ids = request_id_keys(&pending);
        for (_, request_id) in pending {
            reject_disconnected_tool_with_warning(self, session, request_id).await;
        }
        rejected_request_ids
    }

    /// Pure mid-turn follow-ups reclaim a session that still owns Claude tool
    /// calls from the prior segment. Reject those calls so the provider can
    /// accept the new user turn instead of waiting forever for tool_result.
    pub(in crate::anthropic) async fn settle_abandoned_pending_tools(&self, session: &Session) {
        if session.pending_tools.lock().await.is_empty() {
            return;
        }
        tracing::info!(
            thread_id = %session.thread_id,
            "settling abandoned pending tools before pure mid-turn follow-up"
        );
        let _ = self.reject_pending_disconnected_tools(session).await;
    }

    pub(super) async fn discard_pending_disconnected_tools(&self, session: &Session) {
        let pending = take_pending_disconnected_tools(session).await;
        self.agent_efforts
            .remove_tool_results(pending.iter().map(|(tool_use_id, _)| tool_use_id.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeSet, HashMap, HashSet},
        sync::Arc,
        time::Instant,
    };

    use serde_json::json;
    use tokio::sync::Mutex;

    use super::*;
    use crate::{
        agent_backend::{AgentBackend, BackendKind, BackendRoute},
        anthropic::Session,
        app_server::events::ThreadEventDispatcher,
    };

    #[tokio::test]
    async fn unsupported_visible_disconnect_discards_pending_tools_when_abort_is_unavailable() {
        let routes = [BackendRoute::new("worker", BackendKind::PiGateway)];
        let bridge =
            Bridge::new_with_backend(AgentBackend::spawn_routes(&routes), "worker".to_owned());
        let session = Arc::new(Session {
            thread_id: "0:thread".to_owned(),
            model: "worker".to_owned(),
            disabled_subagent_models: BTreeSet::new(),
            signature: Arc::from("signature"),
            transcript: Mutex::new(Vec::new()),
            pending_tools: Mutex::new(HashMap::from([("pending".to_owned(), json!(17))])),
            consumed_tool_ids: Mutex::new(HashSet::new()),
            external_tool_names: HashMap::new(),
            launch_availability: Default::default(),
            client_user_id: None,
            claude_session_id: None,
            gate: Arc::new(Mutex::new(())),
            last_activity: std::sync::Mutex::new(Instant::now()),
            pending_since: std::sync::Mutex::new(None),
            turn_progress: Default::default(),
            adopted_thread_id: Default::default(),
            _slot: Arc::clone(&bridge.session_slots)
                .try_acquire_owned()
                .expect("session slot"),
        });

        bridge
            .handle_unsupported_disconnect(
                &session,
                Arc::new(ThreadEventDispatcher::default().subscribe("thread")),
                true,
            )
            .await;

        assert!(
            session.pending_tools.lock().await.is_empty(),
            "visible disconnect must discard tool results before attempting provider abort"
        );
    }

    #[tokio::test]
    async fn settled_disconnect_keeps_a_claude_session_paused_for_send_message_continue() {
        let copilot = crate::pi_gateway::PiGateway::alive_for_test();
        let bridge = Bridge::new_with_backend(AgentBackend::pi(copilot), "worker".to_owned());
        let session = Arc::new(Session {
            thread_id: "0:thread".to_owned(),
            model: "worker".to_owned(),
            disabled_subagent_models: BTreeSet::new(),
            signature: Arc::from("signature"),
            transcript: Mutex::new(Vec::new()),
            pending_tools: Mutex::new(HashMap::new()),
            consumed_tool_ids: Mutex::new(HashSet::new()),
            external_tool_names: HashMap::new(),
            launch_availability: Default::default(),
            client_user_id: None,
            claude_session_id: Some("claude-session".to_owned()),
            gate: Arc::new(Mutex::new(())),
            last_activity: std::sync::Mutex::new(Instant::now()),
            pending_since: std::sync::Mutex::new(None),
            turn_progress: Default::default(),
            adopted_thread_id: Default::default(),
            _slot: Arc::clone(&bridge.session_slots)
                .try_acquire_owned()
                .expect("session slot"),
        });
        bridge.sessions.lock().await.push(Arc::clone(&session));
        bridge
            .disconnect_stream_with_policy(
                &session,
                Arc::new(ThreadEventDispatcher::default().subscribe("thread")),
                true,
            )
            .await;
        assert_eq!(bridge.sessions.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn settled_disconnect_removes_a_session_without_a_claude_session_id() {
        let copilot = crate::pi_gateway::PiGateway::alive_for_test();
        let bridge = Bridge::new_with_backend(AgentBackend::pi(copilot), "worker".to_owned());
        let session = Arc::new(Session {
            thread_id: "0:thread".to_owned(),
            model: "worker".to_owned(),
            disabled_subagent_models: BTreeSet::new(),
            signature: Arc::from("signature"),
            transcript: Mutex::new(Vec::new()),
            pending_tools: Mutex::new(HashMap::new()),
            consumed_tool_ids: Mutex::new(HashSet::new()),
            external_tool_names: HashMap::new(),
            launch_availability: Default::default(),
            client_user_id: None,
            claude_session_id: None,
            gate: Arc::new(Mutex::new(())),
            last_activity: std::sync::Mutex::new(Instant::now()),
            pending_since: std::sync::Mutex::new(None),
            turn_progress: Default::default(),
            adopted_thread_id: Default::default(),
            _slot: Arc::clone(&bridge.session_slots)
                .try_acquire_owned()
                .expect("session slot"),
        });
        bridge.sessions.lock().await.push(Arc::clone(&session));
        bridge
            .disconnect_stream_with_policy(
                &session,
                Arc::new(ThreadEventDispatcher::default().subscribe("thread")),
                true,
            )
            .await;
        assert!(bridge.sessions.lock().await.is_empty());
    }
}
