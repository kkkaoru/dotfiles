use std::sync::Arc;

use anyhow::{Result, bail};
use serde_json::json;

use super::super::{
    Bridge, Session,
    content::{ToolResult, take_pending_results},
};
use super::helpers::is_idempotent_task_lifecycle_error;

impl Bridge {
    pub(super) async fn acquire_session_slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        if let Ok(slot) = Arc::clone(&self.session_slots).try_acquire_owned() {
            return Ok(slot);
        }
        self.evict_oldest_idle_session().await;
        match Arc::clone(&self.session_slots).try_acquire_owned() {
            Ok(slot) => Ok(slot),
            Err(_) => bail!(
                "claudex session capacity ({}) is busy",
                super::super::MAX_SESSIONS
            ),
        }
    }

    pub(super) async fn submit_tool_results(
        &self,
        session: &Session,
        results: Vec<ToolResult>,
    ) -> Result<bool> {
        let (responses, completed_ids) = take_pending_results(session, results).await?;
        self.agent_efforts
            .remove_tool_results(completed_ids.iter().map(String::as_str));
        // ACP-bridged Agent/Task has no app-server request; continue via transcript.
        let stateless_pi = self
            .app_for_session(session)
            .backend_kind_for_model(&session.model)
            == Some(crate::agent_backend::BackendKind::PiGateway);
        if stateless_pi {
            return Ok(false);
        }
        let mut backend_submitted = false;
        let responses = responses.into_iter().filter(|(id, _)| {
            !crate::anthropic::stream::acp_tool_bridge::is_acp_bridge_request_id(id)
        });
        for (id, result) in responses {
            let success =
                !result.is_error || is_idempotent_task_lifecycle_error(&result.content_items);
            self.app_for_session(session)
                .respond_for_model(
                    &session.model,
                    id,
                    json!({"contentItems": result.content_items, "success": success}),
                )
                .await?;
            backend_submitted = true;
        }
        Ok(backend_submitted)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::{collections::HashMap, sync::Arc, time::Instant};

    use serde_json::json;
    use tokio::sync::{Mutex, Semaphore};

    use super::*;
    use crate::{
        agent_backend::{AgentBackend, BackendKind, BackendRoute},
        anthropic::content::ToolResult,
    };

    #[tokio::test]
    async fn submit_tool_results_targets_the_owning_claude_session_for_provider_models() {
        assert_all_provider_models_target_owning_session().await;
    }

    async fn assert_all_provider_models_target_owning_session() {
        for model in [
            "glm-5.2:cloud",
            "gpt-5.4",
            "grok-4-latest",
            "auto",
            "fugu",
            "cursor-agent",
        ] {
            assert_submit_targets_owning_session(model).await;
        }
    }

    /// A `SessionScoped` backend with two Claude-session pools must route tool
    /// results to the pool that owns the session, never to the anonymous one.
    async fn assert_submit_targets_owning_session(model: &str) {
        let backend =
            AgentBackend::spawn_routes(&[BackendRoute::new(model, BackendKind::CodexAppServer)]);
        let AgentBackend::SessionScoped(scopes) = backend.as_ref() else {
            panic!("expected SessionScoped backends");
        };
        for id in ["tui-a", "tui-b"] {
            let leaf = Arc::new(AgentBackend::Grok(
                crate::grok_acp::GrokAcp::alive_for_test(),
            ));
            scopes.insert_scope_for_test(id, AgentBackend::routed(vec![(model.to_owned(), leaf)]));
        }
        let _ = scopes.scope(None);
        let bridge = Bridge::new_with_backend(Arc::clone(&backend), model.to_owned());
        let session = session_for(model, "tui-a");
        session
            .pending_tools
            .lock()
            .await
            .insert("tool-1".to_owned(), json!(42));
        let error = bridge
            .submit_tool_results(
                &session,
                vec![ToolResult {
                    tool_use_id: "tool-1".to_owned(),
                    content_items: vec![json!({"type":"inputText","text":"ok"})],
                    is_error: false,
                }],
            )
            .await
            .expect_err("Grok leaf rejects Claude tool results");
        let message = error.to_string();
        assert!(
            !message.contains("not initialized"),
            "{model}: Bridge must not send tool results to `_anonymous`: {message}"
        );
        assert!(
            message.contains("Grok ACP"),
            "{model}: expected the owning Claude-session pool: {message}"
        );
    }

    fn session_for(model: &str, claude_session_id: &str) -> Session {
        let slots = Arc::new(Semaphore::new(1));
        Session {
            thread_id: "0:thread".to_owned(),
            model: model.to_owned(),
            disabled_subagent_models: Default::default(),
            signature: Arc::from("signature"),
            transcript: Mutex::new(Vec::new()),
            pending_tools: Mutex::new(HashMap::new()),
            consumed_tool_ids: Mutex::new(Default::default()),
            external_tool_names: HashMap::new(),
            launch_availability: Default::default(),
            client_user_id: None,
            claude_session_id: Some(claude_session_id.to_owned()),
            gate: Arc::new(Mutex::new(())),
            last_activity: std::sync::Mutex::new(Instant::now()),
            pending_since: std::sync::Mutex::new(None),
            turn_progress: Default::default(),
            adopted_thread_id: Default::default(),
            _slot: slots.try_acquire_owned().expect("session slot"),
        }
    }
}
