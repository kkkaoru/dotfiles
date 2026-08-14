use std::sync::Arc;

use anyhow::Result;

use super::super::{
    ActiveTurn, Bridge, MessagesRequest, SelectedSession,
    content::{ToolResult, attach_mid_turn_steering, mid_turn_user_steering},
};

mod recover;
mod retry;
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
        self.app_for_session(&selected.session)
            .ensure_thread_ready(&selected.session.thread_id)
            .await?;
        // Idle sessions can still own pending Claude tools from the prior
        // segment. A pure follow-up (no tool_result payload) must settle them
        // before start_model_turn, or the provider stays blocked on the old call.
        if !has_tool_results {
            self.settle_abandoned_pending_tools(&selected.session).await;
        }
        let events = Arc::new(
            self.app_for_session(&selected.session)
                .subscribe_thread(&selected.session.thread_id),
        );
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
}

#[cfg(test)]
pub(in crate::anthropic) use turn_model::contains_context_window_marker;
pub(crate) use turn_model::is_unknown_session_text;
pub(in crate::anthropic) use turn_model::{
    is_context_window_exceeded, is_unknown_session_exceeded,
};
