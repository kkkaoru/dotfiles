use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::{
    ActiveTurn, BRIDGE_INSTRUCTIONS, Bridge, MessagesRequest, SelectedSession, Session,
    content::{
        ToolResult, collect_tool_results, full_transcript_input, matching_transcript_len,
        request_signature, system_text, take_pending_results, user_input_from_messages,
    },
};
use crate::app_server::response_thread_id;

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
        let extras = request.messages[existing_len..].to_vec();
        let events = self.app.subscribe_thread(&selected.session.thread_id);
        let start = if tool_results.is_empty() {
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
        let response_model = if request.model.is_empty() {
            self.model.clone()
        } else {
            request.model.clone()
        };
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
            "model": self.model
        });
        if let Some(effort) = effort {
            params["effort"] = json!(effort);
        }
        self.app.request_detached("turn/start", params).await
    }

    async fn select_session(
        &self,
        request: &MessagesRequest,
        signature: String,
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
        tool_results: &[ToolResult],
    ) -> Result<SelectedSession> {
        if !tool_results.is_empty() {
            return self.select_pending_session(request, tool_results).await;
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
            gate,
        })
    }

    async fn select_pending_session(
        &self,
        request: &MessagesRequest,
        tool_results: &[ToolResult],
    ) -> Result<SelectedSession> {
        let session = self
            .find_result_session(tool_results)
            .await
            .context("no active claudex session owns the returned Claude tool_use_id")?;
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
        Ok(SelectedSession {
            session,
            existing_len: request.messages.len().saturating_sub(1),
            gate,
        })
    }

    async fn select_matching_session(
        &self,
        signature: &str,
        messages: &[Value],
    ) -> Option<SelectedSession> {
        let (session, _) = self.find_session(signature, messages).await?;
        let gate = Arc::clone(&session.gate).lock_owned().await;
        let existing_len = matching_transcript_len(&session, messages).await?;
        touch_session(&session);
        Some(SelectedSession {
            session,
            existing_len,
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
        signature: &str,
        messages: &[Value],
    ) -> Option<(Arc<Session>, usize)> {
        let sessions = self.sessions.lock().await.clone();
        let mut best = None;
        for session in sessions {
            let Some(length) = candidate_length(&session, signature, messages).await else {
                continue;
            };
            if best.as_ref().is_none_or(|(_, best_len)| length > *best_len) {
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
        signature: String,
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
    ) -> Result<Arc<Session>> {
        let slot = self.acquire_session_slot().await?;
        let (dynamic_tools, external_tool_names, internal_tools) =
            tool_configuration(request, advisor_model, collaborator_model);
        let params = thread_start_params(request, &self.model, dynamic_tools);
        let result = self.app.request("thread/start", params).await?;
        let session = Arc::new(Session {
            thread_id: response_thread_id(&result)?,
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
        Arc::clone(&self.session_slots)
            .try_acquire_owned()
            .map_err(|_| {
                anyhow::anyhow!("claudex session capacity ({}) is busy", super::MAX_SESSIONS)
            })
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
                .respond(
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

async fn candidate_length(
    session: &Arc<Session>,
    signature: &str,
    messages: &[Value],
) -> Option<usize> {
    if session.signature != signature {
        return None;
    }
    matching_transcript_len(session, messages).await
}

fn tool_configuration(
    request: &MessagesRequest,
    advisor_model: Option<&str>,
    collaborator_model: Option<&str>,
) -> (Vec<Value>, HashMap<String, String>, HashMap<String, String>) {
    let (mut tools, external_names) = external_tools(&request.tools);
    let mut internal = HashMap::new();
    if let Some(model) = advisor_model {
        internal.insert("advisor".to_owned(), model.to_owned());
        tools.push(internal_advisor_tool());
    }
    let has_collaborator = request
        .tools
        .iter()
        .any(|tool| tool["name"] == "claude_collaborator");
    if let Some(model) = collaborator_model.filter(|_| !has_collaborator) {
        internal.insert("claude_collaborator".to_owned(), model.to_owned());
        tools.push(internal_collaborator_tool());
    }
    (tools, external_names, internal)
}

fn external_tools(tools: &[Value]) -> (Vec<Value>, HashMap<String, String>) {
    let mut specs = Vec::new();
    let mut names = HashMap::new();
    for (index, tool) in tools.iter().enumerate() {
        let Some(original_name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        let codex_name = codex_tool_name(original_name, index);
        if let Some(spec) = dynamic_tool(tool, &codex_name) {
            names.insert(codex_name, original_name.to_owned());
            specs.push(spec);
        }
    }
    (specs, names)
}

fn thread_start_params(request: &MessagesRequest, model: &str, dynamic_tools: Vec<Value>) -> Value {
    let system = system_text(&request.system);
    let developer_instructions = super::team_protocol::guidance(&request.tools).map_or_else(
        || BRIDGE_INSTRUCTIONS.to_owned(),
        |guidance| format!("{BRIDGE_INSTRUCTIONS}\n\n{guidance}"),
    );
    let base_instructions = if system.is_empty() {
        developer_instructions.clone()
    } else {
        format!("{system}\n\n{developer_instructions}")
    };
    json!({
        "model": model,
        "cwd": isolated_runtime_cwd(),
        "baseInstructions": base_instructions,
        "developerInstructions": developer_instructions,
        "dynamicTools": dynamic_tools,
        "environments": [],
        "ephemeral": true,
        "approvalPolicy": "never",
        "sandbox": "read-only",
        "personality": "none",
        "config": {
            "web_search": "disabled",
            "features": {
                "apps": false, "multi_agent": false, "shell_tool": false,
                "tool_search": false, "unified_exec": false, "web_search": false
            }
        }
    })
}

pub(super) fn dynamic_tool(tool: &Value, codex_name: &str) -> Option<Value> {
    let original_name = tool.get("name")?.as_str()?;
    Some(json!({
        "type": "function",
        "name": codex_name,
        "description": format!(
            "Claude Code tool `{original_name}`. {}",
            tool.get("description").and_then(Value::as_str).unwrap_or("")
        ),
        "inputSchema": super::agent_effort::tool_schema(original_name,
            tool.get("input_schema").cloned()
                .unwrap_or_else(|| json!({"type":"object"})))
    }))
}

pub(super) fn codex_tool_name(original_name: &str, index: usize) -> String {
    let sanitized = original_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let suffix = format!("_{index}");
    let maximum_name_bytes = 128usize.saturating_sub(3 + suffix.len());
    let stem = &sanitized[..sanitized.len().min(maximum_name_bytes)];
    format!("cc_{stem}{suffix}")
}

fn isolated_runtime_cwd() -> String {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(".cache/claudex/codex-home")
        .to_string_lossy()
        .into_owned()
}

pub(super) fn internal_advisor_tool() -> Value {
    json!({
        "type":"function",
        "name":"advisor",
        "description":"Ask the advisor model configured by Claude Code to independently review the entire conversation and return high-value guidance. It takes no parameters.",
        "inputSchema":{"type":"object","properties":{},"additionalProperties":false}
    })
}

pub(super) fn internal_collaborator_tool() -> Value {
    json!({
        "type":"function",
        "name":"claude_collaborator",
        "description":"Delegate an independent task to the collaborator model configured by Claude Code through the user's Claude subscription. Multiple calls may be issued in parallel.",
        "inputSchema":{
            "type":"object",
            "properties":{"task":{"type":"string","description":"The task for the Claude collaborator."}},
            "required":["task"],
            "additionalProperties":false
        }
    })
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
