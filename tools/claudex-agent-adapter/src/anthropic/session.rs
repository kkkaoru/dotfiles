use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use anyhow::{Result, bail};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::{
    ActiveTurn, Bridge, MessagesRequest, SelectedSession, Session,
    content::{
        ToolResult, collect_tool_results, request_signature, take_pending_results,
        transcript_owns_tool_results,
    },
};
use crate::app_server::response_thread_id;

mod continuation;
mod helpers;
mod preempt;
#[cfg(test)]
pub(super) mod reservation;
#[cfg(not(test))]
mod reservation;
mod session_turn;
mod tools;

use helpers::{
    candidate_length, is_better_length, owns_tool_result, should_preempt_for_context_limit,
    touch_session, validate_tool_result_ownership,
};
pub(in crate::anthropic) use session_turn::is_context_window_exceeded;
#[cfg(test)]
pub(super) use tools::{
    codex_tool_name, dynamic_tool, internal_advisor_tool, internal_collaborator_tool,
    thread_start_params, thread_start_params_for_mode, tool_configuration,
    tool_configuration_for_mode,
};
#[cfg(not(test))]
use tools::{thread_start_params_for_mode, tool_configuration_for_mode};

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
        let tool_results = request
            .messages
            .last()
            .map(|message| collect_tool_results(std::slice::from_ref(message)))
            .unwrap_or_default();
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
        self.start_selected_turn(
            request,
            input_tokens,
            effort,
            selected,
            tool_results,
            advisor_model.as_deref(),
            collaborator_model.as_deref(),
            true,
        )
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

    async fn select_session(
        &self,
        request: &MessagesRequest,
        signature: Arc<str>,
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
        tool_results: &[ToolResult],
    ) -> Result<SelectedSession> {
        if !tool_results.is_empty() {
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
            return Ok(SelectedSession {
                session,
                existing_len: 0,
                recovered: true,
                gate,
            });
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
        self.detached_sessions
            .lock()
            .await
            .retain(|session| !Arc::ptr_eq(session, finished));
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
        for session in sessions {
            let pending = session.pending_tools.lock().await;
            let consumed = session.consumed_tool_ids.lock().await;
            if results
                .iter()
                .all(|result| owns_tool_result(&pending, &consumed, &result.tool_use_id))
            {
                drop(consumed);
                drop(pending);
                return Some(session);
            }
        }
        None
    }

    async fn create_session(
        &self,
        request: &MessagesRequest,
        signature: Arc<str>,
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
    ) -> Result<Arc<Session>> {
        let slot = self.acquire_session_slot().await?;
        let model = self.request_model(request);
        let web_search_mode = self.app.web_search_mode(&model);
        let (dynamic_tools, external_tool_names, internal_tools) = tool_configuration_for_mode(
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
            internal_tools,
            external_tool_names,
            client_user_id: request
                .metadata
                .get("user_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            gate: Arc::new(Mutex::new(())),
            last_activity: std::sync::Mutex::new(Instant::now()),
            pending_since: std::sync::Mutex::new(None),
            _slot: slot,
        });
        self.sessions.lock().await.push(Arc::clone(&session));
        Ok(session)
    }

    async fn acquire_session_slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        if let Ok(slot) = Arc::clone(&self.session_slots).try_acquire_owned() {
            return Ok(slot);
        }
        self.evict_oldest_idle_session().await;
        match Arc::clone(&self.session_slots).try_acquire_owned() {
            Ok(slot) => Ok(slot),
            Err(_) => bail!("claudex session capacity ({}) is busy", super::MAX_SESSIONS),
        }
    }

    async fn submit_tool_results(
        &self,
        session: &Session,
        results: Vec<ToolResult>,
    ) -> Result<bool> {
        let (responses, completed_ids) = take_pending_results(session, results).await?;
        self.agent_efforts
            .remove_tool_results(completed_ids.iter().map(String::as_str));
        let submitted = !responses.is_empty();
        for (id, result) in responses {
            self.app
                .respond_for_model(
                    &session.model,
                    id,
                    json!({
                        "contentItems": result.content_items,
                        "success": !result.is_error
                    }),
                )
                .await?;
        }
        Ok(submitted)
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
