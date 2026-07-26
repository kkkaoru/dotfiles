use anyhow::Result;
use serde_json::{Value, json};

use super::super::{
    content::{
        ToolResult,
    },
    turn_input::{full_transcript_input, user_input_from_messages},
    ActiveTurn, Bridge, MessagesRequest, SelectedSession, Session,
};

impl Bridge {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn start_selected_turn(
        &self,
        request: &MessagesRequest,
        input_tokens: u64,
        effort: Option<String>,
        selected: SelectedSession,
        tool_results: Vec<ToolResult>,
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
        allow_context_retry: bool,
    ) -> Result<ActiveTurn> {
        let mut selected = selected;
        let mut existing_len = selected.existing_len;
        let mut extras = request.messages[existing_len..].to_vec();
        let has_tool_results = !tool_results.is_empty();
        let mut start = if tool_results.is_empty() || selected.recovered {
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
            if is_context_window_exceeded(&error) {
                tracing::warn!(
                    error = %error,
                    thread_id = %selected.session.thread_id,
                    existing_len = existing_len,
                    has_tool_results = has_tool_results,
                    "claudex: retrying with fresh thread after context window exceeded"
                );
                selected = self
                    .start_new_session(request, &selected, advisor_model, collaborator_model)
                    .await?;
                existing_len = 0;
                extras = request.messages.to_vec();
                start = self
                    .start_model_turn(
                        request,
                        &selected.session,
                        existing_len,
                        &extras,
                        effort.as_deref(),
                    )
                    .await;
            } else {
                self.remove_session(&selected.session).await;
                return Err(error);
            }
        }
        if let Err(error) = start {
            self.remove_session(&selected.session).await;
            return Err(error);
        }
        let response_model = self.request_model(request);
        let events = self.app.subscribe_thread(&selected.session.thread_id);
        Ok(ActiveTurn {
            session: selected.session,
            events,
            response_model,
            extras,
            input_tokens,
            retry: allow_context_retry.then(|| super::super::ContextRetry {
                request: request.clone(),
                effort: effort.clone(),
                advisor_model: advisor_model.map(str::to_owned),
                collaborator_model: collaborator_model.map(str::to_owned),
            }),
            gate: selected.gate,
        })
    }

    pub(in crate::anthropic) async fn retry_after_context_window(
        &self,
        retry: super::super::ContextRetry,
        previous: &std::sync::Arc<Session>,
        input_tokens: u64,
    ) -> Result<ActiveTurn> {
        let signature = std::sync::Arc::clone(&previous.signature);
        self.remove_session(previous).await;
        let effort = retry.effort.clone();
        let advisor_model = retry.advisor_model.clone();
        let collaborator_model = retry.collaborator_model.clone();
        let session = self
            .create_session(
                &retry.request,
                signature,
                retry.advisor_model.as_deref(),
                retry.collaborator_model.as_deref(),
            )
            .await?;
        let gate = std::sync::Arc::clone(&session.gate).lock_owned().await;
        self.start_selected_turn(
            &retry.request,
            input_tokens,
            effort,
            SelectedSession {
                session,
                existing_len: 0,
                recovered: false,
                gate,
            },
            Vec::new(),
            advisor_model.as_deref(),
            collaborator_model.as_deref(),
            false,
        )
        .await
    }

    pub(super) async fn start_new_session(
        &self,
        request: &MessagesRequest,
        selected: &SelectedSession,
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
    ) -> Result<SelectedSession> {
        let signature = std::sync::Arc::clone(&selected.session.signature);
        self.remove_session(&selected.session).await;
        let session = self
            .create_session(request, signature, advisor_model, collaborator_model)
            .await?;
        let gate = std::sync::Arc::clone(&session.gate).lock_owned().await;
        Ok(SelectedSession {
            session,
            existing_len: 0,
            recovered: false,
            gate,
        })
    }

    pub(super) async fn start_model_turn(
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
        // Mark interactive user turns so ACP keeps a reserved slot free of SubAgent load.
        if !super::super::agent_effort::is_subagent_request(request) {
            params["priority"] = json!("user");
        }
        self.app.request_detached("turn/start", params).await
    }
}

pub(in crate::anthropic) fn is_context_window_exceeded(error: &anyhow::Error) -> bool {
    contains_context_window_marker(&error.to_string())
}

pub(super) fn contains_context_window_marker(message: &str) -> bool {
    let message = message.to_lowercase();
    const CONTEXT_WINDOW_MARKERS: [&str; 5] = [
        "context window",
        "ran out of room",
        "contextwindowexceeded",
        "context_window_exceeded",
        "context limit",
    ];
    CONTEXT_WINDOW_MARKERS
        .into_iter()
        .any(|marker| message.contains(marker))
}
