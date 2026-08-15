use anyhow::Result;

use super::{
    ActiveTurn, Bridge, MessagesRequest, SelectedSession, Session,
    content::{ToolResult, collect_turn_tool_results, request_signature},
    request_identity,
};

mod continuation;
mod helpers;
mod preempt;
#[cfg(test)]
pub(super) mod reservation;
#[cfg(not(test))]
mod reservation;
mod results;
mod select;
mod session_turn;
mod tools;
use helpers::{first_session_owning_results, should_preempt_for_context_limit};
// Child modules and session_tests reach these through the parent namespace.
#[allow(unused_imports)]
use super::content::transcript_owns_tool_results;
#[allow(unused_imports)]
use helpers::{
    candidate_length, is_better_length, is_idempotent_task_lifecycle_error, touch_session,
    validate_tool_result_ownership,
};
pub(in crate::anthropic) use session_turn::is_context_window_exceeded;
pub(crate) use session_turn::is_unknown_session_text;
#[cfg(test)]
pub(super) use session_turn::pi_claude_request;
pub(in crate::anthropic) use tools::is_main_session_only_tool;
#[cfg(test)]
pub(super) use tools::{
    codex_tool_name, dynamic_tool, thread_start_params, thread_start_params_for_mode,
    tool_configuration, tool_configuration_for_mode,
};

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
        // Both fallbacks live in one settings file. Keep the synchronous read
        // to one operation per turn while retaining live settings semantics;
        // avoid a read altogether when explicit overrides cover both values.
        let needs_advisor_setting = self.advisor_model_override.is_none();
        let needs_collaborator_setting = request.claudex_collaborator_model.is_none()
            && self.collaborator_model_override.is_none();
        let settings =
            (needs_advisor_setting || needs_collaborator_setting).then(|| self.claude_settings());
        let advisor_model = self.advisor_model_override.clone().or_else(|| {
            settings
                .as_ref()
                .and_then(|settings| settings.get("advisorModel"))
        });
        let collaborator_model = request
            .claudex_collaborator_model
            .clone()
            .or_else(|| self.collaborator_model_override.clone())
            .or_else(|| settings.as_ref().and_then(|settings| settings.get("model")));
        let signature = request_signature(
            request,
            advisor_model.as_deref(),
            collaborator_model.as_deref(),
        )?;
        let signature =
            self.intern_signature(format!("{}\0{signature}", self.request_model(request)));
        let tool_results = collect_turn_tool_results(&request.messages);
        let mut selected = self
            .select_session(
                request,
                signature,
                advisor_model.as_deref(),
                collaborator_model.as_deref(),
                &tool_results,
            )
            .await?;
        let context_limit = self
            .app_for(request_identity::claude_session_id(request).as_deref())
            .max_context_tokens_for_model(&model);
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
        self.start_selected_turn(session_turn::StartSelectedTurn {
            request,
            input_tokens,
            effort,
            selected,
            tool_results,
            advisor_model: advisor_model.as_deref(),
            collaborator_model: collaborator_model.as_deref(),
            allow_context_retry: true,
        })
        .await
    }
}

#[path = "session_lifecycle.rs"]
mod lifecycle;

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
