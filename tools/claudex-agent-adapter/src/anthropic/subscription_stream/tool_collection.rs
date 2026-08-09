use std::convert::Infallible;

use anyhow::{Context, Result, bail};
use axum::body::Bytes;
use serde_json::Value;
use tokio::sync::mpsc;

use super::SubscriptionStream;
use crate::anthropic::agent_effort::BLOCKED_SUBAGENT_NOTICE;
use crate::anthropic::subscription_frames::{
    mapped_tool_name, send_text_delta, send_text_start, send_tool_block,
};
use crate::anthropic::task_ids::{
    is_claude_code_agent_task_id, is_task_stop_tool_name, skipped_foreign_task_stop_notice,
};

impl SubscriptionStream {
    pub(super) async fn forward_tool_uses(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        envelope: &Value,
    ) -> Result<bool> {
        if self.saw_result || self.blocked_subagent {
            return Ok(false);
        }
        if envelope
            .get("parent_tool_use_id")
            .is_some_and(|value| !value.is_null())
        {
            return Ok(false);
        }
        let Some(content) = envelope
            .pointer("/message/content")
            .and_then(Value::as_array)
        else {
            return Ok(false);
        };
        let tool_uses = content
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
            .collect::<Vec<_>>();
        if tool_uses.is_empty() {
            return Ok(false);
        }
        let closed_visible = self.activity.is_open() || (self.text_started && !self.text_closed);
        self.activity.close(sender).await?;
        self.close_text(sender).await?;
        let mut forwarded = false;
        let mut tool_uses = tool_uses.into_iter();
        while let (false, Some(block)) = (self.blocked_subagent, tool_uses.next()) {
            forwarded |= self.forward_tool_use(sender, block).await?;
        }
        if forwarded {
            self.saw_tool_use = true;
        }
        Ok(closed_visible || forwarded || self.blocked_subagent)
    }

    async fn forward_tool_use(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        block: &Value,
    ) -> Result<bool> {
        let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
        let emitted_name = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let name = mapped_tool_name(emitted_name, &self.tools).to_owned();
        if id.is_empty() || name.is_empty() {
            bail!("Claude subscription emitted a tool call without an ID or name");
        }
        if !self.seen_tool_ids.insert(id.to_owned()) {
            return Ok(false);
        }
        if matches!(name.as_str(), "WebSearch" | "WebFetch" | "StructuredOutput") {
            return Ok(false);
        }
        let input = block
            .get("input")
            .filter(|input| input.is_object())
            .cloned()
            .context("Claude subscription emitted non-object tool input")?;
        let public_input = match self.prepare_tool_input(&name, id, &input) {
            Ok(input) => input,
            Err(error) if super::super::agent_effort::is_agent_tool(&name) => {
                tracing::warn!(%error, tool = name, "blocked unsupported SubAgent launch");
                self.blocked_subagent = true;
                let notice = error.to_string();
                let notice = if notice.contains("cooling down") {
                    notice
                } else {
                    BLOCKED_SUBAGENT_NOTICE.to_owned()
                };
                self.report_blocked_subagent(sender, &notice).await?;
                // The notice is ordinary assistant text, not a tool_use block.
                // Marking it as forwarded would emit stop_reason=tool_use with
                // no corresponding tool block, leaving Claude Code waiting for
                // a tool result and eventually stalling the response stream.
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        if is_task_stop_tool_name(&name) {
            let task_id = public_input
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !is_claude_code_agent_task_id(task_id) {
                tracing::info!(task_id, "skipping TaskStop for non-agent task id");
                if !self.saw_tool_use {
                    self.report_skipped_task_stop(sender, task_id).await?;
                }
                return Ok(false);
            }
        }
        self.report_subagent_action(sender, &name, &input).await?;
        send_tool_block(sender, self.next_index, id, &name, public_input).await?;
        self.next_index += 1;
        self.saw_tool_use = true;
        self.launch_fanout_open = crate::anthropic::agent_effort::is_agent_tool(&name);
        Ok(true)
    }

    async fn report_skipped_task_stop(
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

    async fn report_blocked_subagent(
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
