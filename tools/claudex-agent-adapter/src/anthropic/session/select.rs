use std::sync::Arc;

use anyhow::{Result, bail};
use serde_json::Value;

use super::super::{
    Bridge, MessagesRequest, SelectedSession,
    content::{ToolResult, transcript_owns_tool_results},
};
use super::{
    continuation,
    helpers::{touch_session, validate_tool_result_ownership},
    preempt,
};

impl Bridge {
    pub(super) async fn select_session(
        &self,
        request: &MessagesRequest,
        signature: Arc<str>,
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
        tool_results: &[ToolResult],
    ) -> Result<SelectedSession> {
        if !tool_results.is_empty() {
            return self
                .select_tool_result_session(
                    request,
                    signature,
                    advisor_model,
                    collaborator_model,
                    tool_results,
                )
                .await;
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

    async fn select_tool_result_session(
        &self,
        request: &MessagesRequest,
        signature: Arc<str>,
        advisor_model: Option<&str>,
        collaborator_model: Option<&str>,
        tool_results: &[ToolResult],
    ) -> Result<SelectedSession> {
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
        Ok(SelectedSession {
            session,
            existing_len: 0,
            recovered: true,
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
}

#[path = "select_create.rs"]
mod create;
