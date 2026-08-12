use std::convert::Infallible;

use anyhow::Result;
use axum::body::Bytes;
use serde_json::Value;
use tokio::sync::mpsc;

use super::SubscriptionStream;
use crate::anthropic::subscription_frames::{send_text_delta, send_text_start};
use crate::anthropic::task_ids::{
    is_claude_code_agent_task_id, is_task_output_tool_name, is_task_stop_tool_name,
    skipped_foreign_task_stop_notice, stale_task_output_notice,
};

impl SubscriptionStream {
    pub(super) fn skip_duplicate_subagent_launch(
        &self,
        name: &str,
        id: &str,
        input: &Value,
        public_input: &Value,
    ) -> Result<bool> {
        if !crate::anthropic::agent_effort::is_agent_tool(name) {
            return Ok(false);
        }
        let Some(context) = self.tool_context.as_ref() else {
            return Ok(false);
        };
        let Some(session_id) = context.session_id.as_deref() else {
            return Ok(false);
        };
        let resume = public_input
            .get("resume")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty());
        if resume {
            return Ok(false);
        }
        if context.subagent_reuse.scope_is_occupied(session_id, input) {
            tracing::info!(
                session_id,
                tool = name,
                tool_use_id = id,
                "skipped duplicate same-scope same-model SubAgent launch"
            );
            return Ok(true);
        }
        if !context
            .subagent_reuse
            .note_inflight_launch(session_id, input, id)
        {
            tracing::info!(
                session_id,
                tool = name,
                tool_use_id = id,
                "skipped SubAgent launch because admission claim was taken"
            );
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) async fn skip_foreign_task_stop(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        name: &str,
        public_input: &Value,
    ) -> Result<bool> {
        if !is_task_stop_tool_name(name) {
            return Ok(false);
        }
        let task_id = public_input
            .get("task_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        if is_claude_code_agent_task_id(task_id) {
            return Ok(false);
        }
        tracing::info!(task_id, "skipping TaskStop for non-agent task id");
        if !self.saw_tool_use {
            self.report_skipped_task_stop(sender, task_id).await?;
        }
        Ok(true)
    }

    pub(super) async fn skip_stale_task_output(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        name: &str,
        public_input: &Value,
    ) -> Result<bool> {
        if !is_task_output_tool_name(name) {
            return Ok(false);
        }
        let live_ids = self
            .tool_context
            .as_ref()
            .map(|context| {
                crate::anthropic::subagent_reuse::live_agent_task_ids(&context.user_messages)
            })
            .unwrap_or_default();
        let Some(notice) = stale_task_output_notice(public_input, &live_ids) else {
            return Ok(false);
        };
        tracing::info!(
            task_id = crate::anthropic::task_ids::task_output_id(public_input),
            "skipping TaskOutput for unknown Agent task id"
        );
        if !self.saw_tool_use {
            self.report_blocked_subagent(sender, &notice).await?;
        }
        Ok(true)
    }

    pub(super) async fn report_skipped_task_stop(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        task_id: &str,
    ) -> Result<()> {
        self.close_text(sender).await?;
        send_text_start(sender, self.next_index).await?;
        self.text_started = true;
        self.text_closed = false;
        self.next_index += 1;
        send_text_delta(
            sender,
            self.next_index.saturating_sub(1),
            &skipped_foreign_task_stop_notice(task_id),
        )
        .await
    }

    pub(super) async fn report_blocked_subagent(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        notice: &str,
    ) -> Result<()> {
        // A preceding tool can have advanced `next_index` past the last text
        // block while leaving `text_started` set. Never append the blocked
        // notice to that index: Claude Code will classify it as tool input and
        // reject the following text_delta as "not a text block".
        self.close_text(sender).await?;
        send_text_start(sender, self.next_index).await?;
        self.text_started = true;
        self.text_closed = false;
        self.next_index += 1;
        send_text_delta(sender, self.next_index.saturating_sub(1), notice).await
    }
}
