use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;

use super::super::super::{
    ActiveTurn, Bridge, MessagesRequest, SelectedSession, Session, content::ToolResult,
};
use super::StartSelectedTurn;

impl Bridge {
    pub(super) async fn kick_off_selected_turn(
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
        retry: super::super::super::ContextRetry,
        previous: &Arc<Session>,
        input_tokens: u64,
    ) -> Result<ActiveTurn> {
        let signature = Arc::clone(&previous.signature);
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
        let gate = Arc::clone(&session.gate).lock_owned().await;
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

    pub(in crate::anthropic::session) async fn start_new_session(
        &self,
        request: &MessagesRequest,
        selected: &SelectedSession,
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
    ) -> Result<SelectedSession> {
        let signature = Arc::clone(&selected.session.signature);
        self.remove_session(&selected.session).await;
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
}
