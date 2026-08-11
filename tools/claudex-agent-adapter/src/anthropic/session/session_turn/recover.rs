use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;

use super::super::super::{Bridge, SelectedSession};
use super::{StartContextRetry, is_context_window_exceeded};

impl Bridge {
    pub(in crate::anthropic) async fn recover_turn_start(
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

    pub(in crate::anthropic) async fn finish_turn_start(
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

    pub(in crate::anthropic) async fn restart_after_start_context_error(
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
}
