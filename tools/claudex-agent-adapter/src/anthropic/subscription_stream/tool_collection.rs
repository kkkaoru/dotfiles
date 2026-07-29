use std::convert::Infallible;

use anyhow::{Context, Result, bail};
use axum::body::Bytes;
use serde_json::Value;
use tokio::sync::mpsc;

use super::SubscriptionStream;
use crate::anthropic::subscription_frames::{
    mapped_tool_name, send_text_delta, send_text_start, send_tool_block,
};

const BLOCKED_SUBAGENT_NOTICE: &str =
    "The requested SubAgent model is not configured, so it was not started. Continue without it.";

impl SubscriptionStream {
    pub(super) async fn forward_tool_uses(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        envelope: &Value,
    ) -> Result<()> {
        if envelope
            .get("parent_tool_use_id")
            .is_some_and(|value| !value.is_null())
        {
            return Ok(());
        }
        let Some(content) = envelope
            .pointer("/message/content")
            .and_then(Value::as_array)
        else {
            return Ok(());
        };
        let tool_uses = content
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
            .collect::<Vec<_>>();
        if tool_uses.is_empty() {
            return Ok(());
        }
        self.activity.close(sender).await?;
        self.close_text(sender).await?;
        let mut forwarded = false;
        for block in tool_uses {
            forwarded |= self.forward_tool_use(sender, block).await?;
        }
        self.saw_tool_use = forwarded;
        Ok(())
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
        let name = mapped_tool_name(emitted_name, &self.tools);
        if id.is_empty() || name.is_empty() {
            bail!("Claude subscription emitted a tool call without an ID or name");
        }
        let input = block
            .get("input")
            .filter(|input| input.is_object())
            .cloned()
            .context("Claude subscription emitted non-object tool input")?;
        let public_input = match self.prepare_tool_input(name, id, &input) {
            Ok(input) => input,
            Err(error) if super::super::agent_effort::is_agent_tool(name) => {
                tracing::warn!(%error, tool = name, "blocked unsupported SubAgent launch");
                self.report_blocked_subagent(sender).await?;
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        send_tool_block(sender, self.next_index, id, name, public_input).await?;
        self.next_index += 1;
        Ok(true)
    }

    async fn report_blocked_subagent(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    ) -> Result<()> {
        if !self.text_started {
            send_text_start(sender, self.next_index).await?;
            self.text_started = true;
            self.next_index += 1;
        }
        send_text_delta(
            sender,
            self.next_index.saturating_sub(1),
            BLOCKED_SUBAGENT_NOTICE,
        )
        .await
    }
}
