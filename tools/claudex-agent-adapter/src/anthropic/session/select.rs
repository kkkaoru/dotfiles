use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use anyhow::{Result, bail};
use serde_json::Value;
use tokio::sync::Mutex;

use super::super::{
    Bridge, MessagesRequest, SelectedSession, Session,
    content::{ToolResult, transcript_owns_tool_results},
};
use super::{
    continuation, preempt,
    helpers::{touch_session, validate_tool_result_ownership},
    tools::{thread_start_params_for_mode, tool_configuration_for_mode},
};
use crate::app_server::response_thread_id;

impl Bridge {
    pub(super) async fn select_session(
        &self,
        request: &MessagesRequest,
        signature: Arc<str>,
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
        tool_results: &[ToolResult],
    ) -> Result<SelectedSession> {
        if !tool_results.is_empty() {
            return self
                .select_tool_result_session(
                    request,
                    signature,
                    advisor_model,
                    collaborator_model,
                    tool_results,
                )
                .await;
        }
        if let Some(selected) = self
            .select_matching_session(request, &signature, &request.messages)
            .await
        {
            return Ok(selected);
        }
        // Claude Code can omit the unchanged tool schemas on an ordinary
        // resumed main or SubAgent request. The schemas belong to the provider
        // thread, so cold-starting here would replace the previous complete
        // capability set (for example `Bash`) with only the fallback tools.
        // Reuse only a tool-less continuation whose model, client identity,
        // and transcript all match an idle session.
        if request.tools.is_empty()
            && let Some(selected) = continuation::select_toolless_main_session(self, request).await
        {
            return Ok(selected);
        }
        let session = self
            .create_session(request, signature, advisor_model, collaborator_model)
            .await?;
        let gate = Arc::clone(&session.gate).lock_owned().await;
        Ok(SelectedSession {
            session,
            existing_len: 0,
            recovered: false,
            gate,
        })
    }

    async fn select_tool_result_session(
        &self,
        request: &MessagesRequest,
        signature: Arc<str>,
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
        tool_results: &[ToolResult],
    ) -> Result<SelectedSession> {
        if let Some(selected) = self.select_pending_session(request, tool_results).await? {
            return Ok(selected);
        }
        if !transcript_owns_tool_results(&request.messages, tool_results) {
            bail!("no active claudex session owns the returned Claude tool_use_id");
        }
        tracing::warn!(
            tool_result_count = tool_results.len(),
            "recovering Claude tool results after adapter session loss"
        );
        let session = self
            .create_session(request, signature, advisor_model, collaborator_model)
            .await?;
        let gate = Arc::clone(&session.gate).lock_owned().await;
        Ok(SelectedSession {
            session,
            existing_len: 0,
            recovered: true,
            gate,
        })
    }

    async fn select_pending_session(
        &self,
        request: &MessagesRequest,
        tool_results: &[ToolResult],
    ) -> Result<Option<SelectedSession>> {
        let Some(session) = self.find_result_session(tool_results).await else {
            return Ok(None);
        };
        let gate = Arc::clone(&session.gate).lock_owned().await;
        let pending = session.pending_tools.lock().await;
        let consumed = session.consumed_tool_ids.lock().await;
        let valid = validate_tool_result_ownership(&pending, &consumed, tool_results);
        drop(consumed);
        drop(pending);
        valid?;
        touch_session(&session);
        Ok(Some(SelectedSession {
            session,
            existing_len: request.messages.len().saturating_sub(1),
            recovered: false,
            gate,
        }))
    }

    async fn select_matching_session(
        &self,
        request: &MessagesRequest,
        signature: &Arc<str>,
        messages: &[Value],
    ) -> Option<SelectedSession> {
        let sessions = self.sessions.lock().await.clone();
        preempt::select_matching_session(sessions, request, signature, messages, &self.app).await
    }

    pub(super) async fn create_session(
        &self,
        request: &MessagesRequest,
        signature: Arc<str>,
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
    ) -> Result<Arc<Session>> {
        let slot = self.acquire_session_slot().await?;
        let model = self.request_model(request);
        let web_search_mode = self.app.web_search_mode(&model);
        let (dynamic_tools, external_tool_names, _internal_tools) = tool_configuration_for_mode(
            request,
            advisor_model,
            collaborator_model,
            web_search_mode,
        );
        let params = thread_start_params_for_mode(request, &model, dynamic_tools, web_search_mode);
        let result = self.app.request("thread/start", params).await?;
        let session = Arc::new(Session {
            thread_id: response_thread_id(&result)?,
            model,
            disabled_subagent_models: request.disabled_subagent_models.clone(),
            signature,
            transcript: Mutex::new(Vec::new()),
            pending_tools: Mutex::new(HashMap::new()),
            consumed_tool_ids: Mutex::new(HashSet::new()),
            external_tool_names,
            client_user_id: request
                .metadata
                .get("user_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            claude_session_id: super::super::request_identity::claude_session_id(request),
            gate: Arc::new(Mutex::new(())),
            last_activity: std::sync::Mutex::new(Instant::now()),
            pending_since: std::sync::Mutex::new(None),
            _slot: slot,
        });
        self.sessions.lock().await.push(Arc::clone(&session));
        Ok(session)
    }
}
