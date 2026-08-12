use std::convert::Infallible;

use anyhow::{Context, Result, bail};
use axum::body::Bytes;
use serde_json::Value;
use tokio::sync::mpsc;

use super::SubscriptionStream;
use crate::anthropic::agent_effort::BLOCKED_SUBAGENT_NOTICE;
use crate::anthropic::subscription_frames::{mapped_tool_name, send_tool_block};

#[path = "tool_collection_skip.rs"]
mod skip;

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
        let closed_visible = self.activity.is_open()
            || self.thinking_open.is_some()
            || (self.text_started && !self.text_closed);
        self.activity.close(sender).await?;
        self.close_native_thinking(sender).await?;
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
        let (private_input, public_input) = match self
            .prepare_routed_tool_input(sender, &name, id, &input)
            .await?
        {
            Some(inputs) => inputs,
            None => return Ok(false),
        };
        if self.skip_duplicate_subagent_launch(&name, id, &private_input, &public_input)? {
            return Ok(false);
        }
        if self
            .skip_foreign_task_stop(sender, &name, &public_input)
            .await?
        {
            return Ok(false);
        }
        if self
            .skip_stale_task_output(sender, &name, &public_input)
            .await?
        {
            return Ok(false);
        }
        self.report_subagent_action(sender, &name, &private_input)
            .await?;
        send_tool_block(sender, self.next_index, id, &name, public_input).await?;
        self.next_index += 1;
        self.saw_tool_use = true;
        if crate::anthropic::agent_effort::is_agent_tool(&name) {
            self.arm_launch_fanout();
        } else {
            self.clear_launch_fanout();
        }
        Ok(true)
    }

    async fn prepare_routed_tool_input(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        name: &str,
        id: &str,
        input: &Value,
    ) -> Result<Option<(Value, Value)>> {
        match self.route_agent_tool_input(name, id, input) {
            Ok(inputs) => Ok(Some(inputs)),
            Err(error) if crate::anthropic::agent_effort::is_agent_tool(name) => {
                self.emit_blocked_subagent(sender, name, error).await?;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    async fn emit_blocked_subagent(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        name: &str,
        error: anyhow::Error,
    ) -> Result<()> {
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
        Ok(())
    }
}
