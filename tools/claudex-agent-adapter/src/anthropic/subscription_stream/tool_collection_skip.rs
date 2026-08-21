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

const UNLISTED_SEND_MESSAGE_NOTICE: &str =
    "Tool `SendMessage` was not supplied by Claude Code and was not executed.";
const DUPLICATE_SUBAGENT_NOTICE: &str = "A same-scope SubAgent is already running, so this duplicate launch was not started. Continue with the existing worker.";
const LIVE_SUBAGENT_CAP_NOTICE: &str = "The live Agent cap was reached, including nested workers, so a new Agent/Task launch was not started. Continue an existing worker with SendMessage({to}) if that tool is listed.";

enum OccupiedFollowUp {
    Rewritten,
    Queued,
    Unchanged,
}

impl SubscriptionStream {
    pub(super) async fn skip_duplicate_subagent_launch(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        name: &str,
        id: &str,
        input: &mut Value,
        public_input: &mut Value,
    ) -> Result<bool> {
        if !crate::anthropic::agent_effort::is_agent_tool(name) {
            return Ok(false);
        }
        if crate::anthropic::subagent_reuse::is_send_message_follow_up(public_input) {
            return Ok(false);
        }
        match self.occupied_subagent_follow_up(id, input, public_input) {
            OccupiedFollowUp::Rewritten => return Ok(false),
            OccupiedFollowUp::Queued => return Ok(true),
            OccupiedFollowUp::Unchanged => {}
        }
        let Some((session_id, reason, notice)) =
            self.skipped_subagent_launch_notice(id, input, public_input)
        else {
            return Ok(false);
        };
        self.reject_skipped_subagent(sender, &session_id, name, id, reason, notice)
            .await
    }

    fn occupied_subagent_follow_up(
        &self,
        id: &str,
        input: &mut Value,
        public_input: &mut Value,
    ) -> OccupiedFollowUp {
        let Some(context) = self.tool_context.as_ref() else {
            return OccupiedFollowUp::Unchanged;
        };
        if context.is_subagent || !crate::anthropic::subagent_reuse::reuse_enabled() {
            return OccupiedFollowUp::Unchanged;
        }
        let Some(session_id) = context.session_id.as_deref() else {
            return OccupiedFollowUp::Unchanged;
        };
        if !context.subagent_reuse.scope_is_occupied(session_id, input) {
            return OccupiedFollowUp::Unchanged;
        }
        if context
            .subagent_reuse
            .rewrite_launch_input(session_id, input)
            .is_some()
        {
            *public_input = input.clone();
            return OccupiedFollowUp::Rewritten;
        }
        if context
            .subagent_reuse
            .queue_inflight_follow_up(session_id, input)
        {
            tracing::info!(
                session_id,
                tool_use_id = id,
                "queued follow-up for an inflight same-scope SubAgent"
            );
            return OccupiedFollowUp::Queued;
        }
        OccupiedFollowUp::Unchanged
    }

    fn skipped_subagent_launch_notice(
        &self,
        id: &str,
        input: &Value,
        public_input: &Value,
    ) -> Option<(String, &'static str, &'static str)> {
        let context = self.tool_context.as_ref()?;
        if crate::anthropic::subagent_reuse::is_send_message_follow_up(public_input) {
            return None;
        }
        if context.is_subagent {
            return Some((
                context.session_id.clone().unwrap_or_default(),
                "rejected nested SubAgent Agent/Task launch",
                crate::anthropic::subagent_reuse::NESTED_SUBAGENT_LAUNCH_NOTICE,
            ));
        }
        if !crate::anthropic::subagent_reuse::reuse_enabled() {
            return None;
        }
        let session_id = context.session_id.as_deref()?;
        if context.subagent_reuse.scope_is_occupied(session_id, input) {
            return Some((
                session_id.to_owned(),
                "skipped duplicate same-scope same-model SubAgent launch",
                DUPLICATE_SUBAGENT_NOTICE,
            ));
        }
        if context
            .subagent_reuse
            .session_at_live_capacity(session_id, &context.user_messages)
        {
            return Some((
                session_id.to_owned(),
                "skipped SubAgent launch because the session live Agent cap was reached",
                LIVE_SUBAGENT_CAP_NOTICE,
            ));
        }
        if context
            .subagent_reuse
            .note_inflight_launch(session_id, input, id)
        {
            return None;
        }
        Some((
            session_id.to_owned(),
            "skipped SubAgent launch because admission claim was taken",
            DUPLICATE_SUBAGENT_NOTICE,
        ))
    }

    async fn reject_skipped_subagent(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        session_id: &str,
        name: &str,
        id: &str,
        log: &str,
        notice: &str,
    ) -> Result<bool> {
        tracing::info!(session_id, tool = name, tool_use_id = id, "{log}");
        self.report_blocked_agent_notice(sender, notice).await?;
        Ok(true)
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

    pub(super) async fn reject_unlisted_send_message(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    ) -> Result<()> {
        tracing::warn!(
            "rejected SendMessage follow-up because Claude Code did not list SendMessage"
        );
        self.blocked_subagent = true;
        self.report_blocked_agent_notice(sender, UNLISTED_SEND_MESSAGE_NOTICE)
            .await
    }

    pub(super) async fn report_blocked_agent_notice(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        notice: &str,
    ) -> Result<()> {
        self.activity
            .start_status(sender, notice, &mut self.next_index)
            .await?;
        self.activity.close(sender).await?;
        self.activity.defer_text(notice);
        // The live notice is thinking chrome. The committed text block is
        // opened at finish, after all provider blocks have been closed.
        self.text_started = false;
        self.text_closed = false;
        Ok(())
    }
}
