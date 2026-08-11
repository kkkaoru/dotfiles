use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;

use super::super::{
    ActiveTurn, Bridge, MessagesRequest, SelectedSession, Session,
    content::{ToolResult, attach_mid_turn_steering, mid_turn_user_steering},
};

mod recover;
#[path = "session_turn_model.rs"]
mod turn_model;

pub(in crate::anthropic) struct StartContextRetry<'a> {
    pub(super) request: &'a MessagesRequest,
    pub(super) effort: Option<&'a str>,
    pub(super) advisor_model: Option<&'a str>,
    pub(super) collaborator_model: Option<&'a str>,
    pub(super) has_tool_results: bool,
}

pub(super) struct StartSelectedTurn<'a> {
    pub(super) request: &'a MessagesRequest,
    pub(super) input_tokens: u64,
    pub(super) effort: Option<String>,
    pub(super) selected: SelectedSession,
    pub(super) tool_results: Vec<ToolResult>,
    pub(super) advisor_model: Option<&'a str>,
    pub(super) collaborator_model: Option<&'a str>,
    pub(super) allow_context_retry: bool,
}

impl Bridge {
    pub(super) async fn start_selected_turn(
        &self,
        args: StartSelectedTurn<'_>,
    ) -> Result<ActiveTurn> {
        let StartSelectedTurn {
            request,
            input_tokens,
            effort,
            selected,
            mut tool_results,
            advisor_model,
            collaborator_model,
            allow_context_retry,
        } = args;
        let existing_len = selected.existing_len;
        let extras = request.messages[existing_len..].to_vec();
        let has_tool_results = !tool_results.is_empty();
        if let Some(steering) = mid_turn_user_steering(&request.messages) {
            attach_mid_turn_steering(&mut tool_results, &steering);
        }
        self.app
            .ensure_thread_ready(&selected.session.thread_id)
            .await?;
        // Idle sessions can still own pending Claude tools from the prior
        // segment. A pure follow-up (no tool_result payload) must settle them
        // before start_model_turn, or the provider stays blocked on the old call.
        if !has_tool_results {
            self.settle_abandoned_pending_tools(&selected.session).await;
        }
        let events = Arc::new(self.app.subscribe_thread(&selected.session.thread_id));
        let turn_start = self
            .kick_off_selected_turn(
                request,
                &selected,
                existing_len,
                &extras,
                effort.as_deref(),
                tool_results,
            )
            .await;
        let (selected, extras, events) = self
            .recover_turn_start(
                selected,
                extras,
                events,
                turn_start,
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

    async fn kick_off_selected_turn(
        &self,
        request: &MessagesRequest,
        selected: &SelectedSession,
        existing_len: usize,
        extras: &[Value],
        effort: Option<&str>,
        tool_results: Vec<ToolResult>,
    ) -> Result<()> {
        if tool_results.is_empty() || selected.recovered {
            return self
                .start_model_turn(request, &selected.session, existing_len, extras, effort)
                .await;
        }
        if self
            .submit_tool_results(&selected.session, tool_results)
            .await?
        {
            return Ok(());
        }
        self.start_model_turn(request, &selected.session, existing_len, extras, effort)
            .await
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
            .start_selected_turn(StartSelectedTurn {
                request: &retry.request,
                input_tokens,
                effort,
                selected: SelectedSession {
                    session,
                    existing_len: 0,
                    recovered: false,
                    gate,
                },
                tool_results: Vec::new(),
                advisor_model: advisor_model.as_deref(),
                collaborator_model: collaborator_model.as_deref(),
                allow_context_retry: false,
            })
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
}

pub(in crate::anthropic) use turn_model::is_context_window_exceeded;
#[cfg(test)]
pub(in crate::anthropic) use turn_model::contains_context_window_marker;
