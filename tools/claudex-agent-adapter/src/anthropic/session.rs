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
        ToolResult, collect_tool_results, full_transcript_input, matching_transcript_len,
        request_signature, take_pending_results, transcript_owns_tool_results,
        user_input_from_messages,
    },
};
use crate::app_server::response_thread_id;

mod tools;

#[cfg(test)]
pub(super) use tools::{
    codex_tool_name, dynamic_tool, internal_advisor_tool, internal_collaborator_tool,
};
use tools::{thread_start_params, tool_configuration};

impl Bridge {
    pub(super) async fn prepare_turn(
        &self,
        request: &MessagesRequest,
        input_tokens: u64,
        effort: Option<String>,
    ) -> Result<ActiveTurn> {
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
        let selected = self
            .select_session(
                request,
                signature,
                advisor_model.as_deref(),
                collaborator_model.as_deref(),
                &tool_results,
            )
            .await?;
        self.start_selected_turn(request, input_tokens, effort, selected, tool_results)
            .await
    }

    async fn start_selected_turn(
        &self,
        request: &MessagesRequest,
        input_tokens: u64,
        effort: Option<String>,
        selected: SelectedSession,
        tool_results: Vec<ToolResult>,
    ) -> Result<ActiveTurn> {
        let existing_len = selected.existing_len;
        let recovered = selected.recovered;
        let extras = request.messages[existing_len..].to_vec();
        let events = self.app.subscribe_thread(&selected.session.thread_id);
        let start = if tool_results.is_empty() || recovered {
            self.start_model_turn(
                request,
                &selected.session,
                existing_len,
                &extras,
                effort.as_deref(),
            )
            .await
        } else if self
            .submit_tool_results(&selected.session, tool_results)
            .await?
        {
            Ok(())
        } else {
            self.start_model_turn(
                request,
                &selected.session,
                existing_len,
                &extras,
                effort.as_deref(),
            )
            .await
        };
        if let Err(error) = start {
            self.remove_session(&selected.session).await;
            return Err(error);
        }
        let response_model = self.request_model(request);
        Ok(ActiveTurn {
            session: selected.session,
            events,
            response_model,
            extras,
            input_tokens,
            gate: selected.gate,
        })
    }

    async fn start_model_turn(
        &self,
        request: &MessagesRequest,
        session: &Session,
        existing_len: usize,
        extras: &[Value],
        effort: Option<&str>,
    ) -> Result<()> {
        let input = if existing_len == 0 {
            full_transcript_input(&request.messages)
        } else {
            user_input_from_messages(extras)
        };
        let mut params = json!({
            "threadId": session.thread_id,
            "input": input,
            "model": self.request_model(request)
        });
        if let Some(effort) = effort {
            params["effort"] = json!(effort);
        }
        self.app.request_detached("turn/start", params).await
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
            .select_matching_session(&signature, &request.messages)
            .await
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
        let valid = tool_results
            .iter()
            .all(|result| owns_tool_result(&pending, &consumed, &result.tool_use_id));
        drop(consumed);
        drop(pending);
        if !valid {
            bail!("Claude tool results were already consumed by another request");
        }
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
        signature: &Arc<str>,
        messages: &[Value],
    ) -> Option<SelectedSession> {
        let (session, _) = self.find_session(signature, messages).await?;
        let gate = Arc::clone(&session.gate).lock_owned().await;
        let existing_len = matching_transcript_len(&session, messages).await?;
        touch_session(&session);
        Some(SelectedSession {
            session,
            existing_len,
            recovered: false,
            gate,
        })
    }

    pub(super) async fn remove_session(&self, removed: &Arc<Session>) {
        self.sessions
            .lock()
            .await
            .retain(|session| !Arc::ptr_eq(session, removed));
    }

    async fn find_session(
        &self,
        signature: &Arc<str>,
        messages: &[Value],
    ) -> Option<(Arc<Session>, usize)> {
        let sessions = self.sessions.lock().await.clone();
        let mut best = None;
        for session in sessions {
            let Some(length) = candidate_length(&session, signature, messages).await else {
                continue;
            };
            let best_length = best.as_ref().map(|(_, best_len)| *best_len);
            if is_better_length(best_length, length) {
                best = Some((session, length));
            }
        }
        best
    }

    async fn find_result_session(&self, results: &[ToolResult]) -> Option<Arc<Session>> {
        let sessions = self.sessions.lock().await.clone();
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
        let (dynamic_tools, external_tool_names, internal_tools) =
            tool_configuration(request, advisor_model, collaborator_model);
        let model = self.request_model(request);
        let params = thread_start_params(request, &model, dynamic_tools);
        let result = self.app.request("thread/start", params).await?;
        let session = Arc::new(Session {
            thread_id: response_thread_id(&result)?,
            model,
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
        let responses = take_pending_results(session, results).await?;
        self.agent_efforts.remove_tool_results(
            responses
                .iter()
                .map(|(_, result)| result.tool_use_id.as_str()),
        );
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

fn touch_session(session: &Session) {
    *session
        .last_activity
        .lock()
        .expect("session clock poisoned") = Instant::now();
}

fn owns_tool_result(
    pending: &HashMap<String, Value>,
    consumed: &HashSet<String>,
    tool_use_id: &str,
) -> bool {
    pending.contains_key(tool_use_id) || consumed.contains(tool_use_id)
}

fn is_better_length(best: Option<usize>, candidate: usize) -> bool {
    match best {
        Some(best) => candidate > best,
        None => true,
    }
}

async fn candidate_length(
    session: &Arc<Session>,
    signature: &Arc<str>,
    messages: &[Value],
) -> Option<usize> {
    if !Arc::ptr_eq(&session.signature, signature)
        && session.signature.as_ref() != signature.as_ref()
    {
        return None;
    }
    matching_transcript_len(session, messages).await
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
