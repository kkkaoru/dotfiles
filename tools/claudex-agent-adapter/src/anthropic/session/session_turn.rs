use std::sync::Arc;

use anyhow::Result;
use serde_json::{Value, json};

use super::super::{
    ActiveTurn, Bridge, MessagesRequest, SelectedSession, Session,
    content::{ToolResult, serialized_len},
    turn_input::{
        provider_turn_input, provider_turn_input_with_token_budget, provider_user_turn_input,
    },
};
use super::tools::{thread_start_params_for_mode, tool_configuration_for_mode};

pub(super) struct StartContextRetry<'a> {
    pub(super) request: &'a MessagesRequest,
    pub(super) effort: Option<&'a str>,
    pub(super) advisor_model: Option<&'a str>,
    pub(super) collaborator_model: Option<&'a str>,
    pub(super) has_tool_results: bool,
}

impl Bridge {
    // This method mirrors the complete session-turn state assembled by the
    // request router; explicit fields keep ownership and retry context visible.
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
        let existing_len = selected.existing_len;
        let extras = request.messages[existing_len..].to_vec();
        let has_tool_results = !tool_results.is_empty();
        self.app
            .ensure_thread_ready(&selected.session.thread_id)
            .await?;
        let events = Arc::new(self.app.subscribe_thread(&selected.session.thread_id));
        let start = if tool_results.is_empty() || selected.recovered {
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
        let (selected, extras, events) = self
            .recover_turn_start(
                selected,
                extras,
                events,
                start,
                StartContextRetry {
                    request,
                    effort: effort.as_deref(),
                    advisor_model,
                    collaborator_model,
                    has_tool_results,
                },
            )
            .await?;
        let response_model = self.request_model(request);
        Ok(ActiveTurn {
            session: selected.session,
            events,
            response_model,
            extras,
            routing_system: request.system.clone(),
            input_tokens,
            retry: allow_context_retry.then(|| super::super::ContextRetry {
                request: request.clone(),
                effort: effort.clone(),
                advisor_model: advisor_model.map(str::to_owned),
                collaborator_model: collaborator_model.map(str::to_owned),
            }),
            gate: selected.gate,
            detached: false,
        })
    }

    pub(super) async fn recover_turn_start(
        &self,
        selected: SelectedSession,
        extras: Vec<Value>,
        events: Arc<crate::app_server::ThreadEvents>,
        start: Result<()>,
        context: StartContextRetry<'_>,
    ) -> Result<(
        SelectedSession,
        Vec<Value>,
        Arc<crate::app_server::ThreadEvents>,
    )> {
        let (selected, extras, events, start) = match start {
            Ok(()) => (selected, extras, events, Ok(())),
            Err(error) if !is_context_window_exceeded(&error) => {
                self.remove_session(&selected.session).await;
                return Err(error);
            }
            Err(error) => {
                self.restart_after_start_context_error(selected, context, &error)
                    .await?
            }
        };
        self.finish_turn_start(selected, extras, start)
            .await
            .map(|(selected, extras)| (selected, extras, events))
    }

    pub(super) async fn finish_turn_start(
        &self,
        selected: SelectedSession,
        extras: Vec<Value>,
        start: Result<()>,
    ) -> Result<(SelectedSession, Vec<Value>)> {
        if let Err(error) = start {
            self.remove_session(&selected.session).await;
            return Err(error);
        }
        Ok((selected, extras))
    }

    pub(super) async fn restart_after_start_context_error(
        &self,
        selected: SelectedSession,
        context: StartContextRetry<'_>,
        error: &anyhow::Error,
    ) -> Result<(
        SelectedSession,
        Vec<Value>,
        Arc<crate::app_server::ThreadEvents>,
        Result<()>,
    )> {
        tracing::warn!(
            error = %error,
            thread_id = %selected.session.thread_id,
            existing_len = selected.existing_len,
            has_tool_results = context.has_tool_results,
            "claudex: retrying with fresh thread after context window exceeded"
        );
        let selected = self
            .start_new_session(
                context.request,
                &selected,
                context.advisor_model,
                context.collaborator_model,
            )
            .await?;
        let extras = context.request.messages.to_vec();
        self.app
            .ensure_thread_ready(&selected.session.thread_id)
            .await?;
        let events = Arc::new(self.app.subscribe_thread(&selected.session.thread_id));
        let start = self
            .start_model_turn(
                context.request,
                &selected.session,
                0,
                &extras,
                context.effort,
            )
            .await;
        Ok((selected, extras, events, start))
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
        let detached = self.is_detached_session(previous).await;
        let mut turn = self
            .start_selected_turn(
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
            .await?;
        if detached {
            self.detach_session(&turn.session).await;
            turn.detached = true;
        }
        Ok(turn)
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
            self.transcript_input(request)
        } else {
            provider_user_turn_input(&self.request_model(request), extras)
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

    fn transcript_input(&self, request: &MessagesRequest) -> Vec<Value> {
        let model = self.request_model(request);
        if crate::command_code_acp::is_command_code_model(&model) {
            return provider_user_turn_input(&model, &request.messages);
        }
        let Some(limit) = self.app.max_context_tokens_for_model(&model) else {
            return provider_turn_input(&model, &request.messages);
        };
        let web_search_mode = self.app.web_search_mode(&model);
        let (dynamic_tools, _, _) =
            tool_configuration_for_mode(request, None, None, web_search_mode);
        let start_params =
            thread_start_params_for_mode(request, &model, dynamic_tools, web_search_mode);
        let setup_tokens = serialized_len(&start_params).div_ceil(4);
        let token_budget = usize::try_from(limit)
            .unwrap_or(usize::MAX)
            .saturating_sub(setup_tokens);
        provider_turn_input_with_token_budget(&model, &request.messages, token_budget)
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
