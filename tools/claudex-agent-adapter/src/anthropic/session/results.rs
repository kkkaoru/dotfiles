use std::sync::Arc;

use anyhow::{Result, bail};
use serde_json::json;

use super::super::{
    Bridge, Session,
    content::{ToolResult, take_pending_results},
};
use super::helpers::is_idempotent_task_lifecycle_error;

impl Bridge {
    pub(super) async fn acquire_session_slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        if let Ok(slot) = Arc::clone(&self.session_slots).try_acquire_owned() {
            return Ok(slot);
        }
        self.evict_oldest_idle_session().await;
        match Arc::clone(&self.session_slots).try_acquire_owned() {
            Ok(slot) => Ok(slot),
            Err(_) => bail!(
                "claudex session capacity ({}) is busy",
                super::super::MAX_SESSIONS
            ),
        }
    }

    pub(super) async fn submit_tool_results(
        &self,
        session: &Session,
        results: Vec<ToolResult>,
    ) -> Result<bool> {
        let (responses, completed_ids) = take_pending_results(session, results).await?;
        self.agent_efforts
            .remove_tool_results(completed_ids.iter().map(String::as_str));
        // ACP-bridged Agent/Task has no app-server request; continue via transcript.
        let mut backend_submitted = false;
        let responses = responses.into_iter().filter(|(id, _)| {
            !crate::anthropic::stream::acp_tool_bridge::is_acp_bridge_request_id(id)
        });
        for (id, result) in responses {
            let success =
                !result.is_error || is_idempotent_task_lifecycle_error(&result.content_items);
            self.app
                .respond_for_model(
                    &session.model,
                    id,
                    json!({"contentItems": result.content_items, "success": success}),
                )
                .await?;
            backend_submitted = true;
        }
        Ok(backend_submitted)
    }
}
