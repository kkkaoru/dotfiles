use std::convert::Infallible;

use anyhow::{Context, Result, bail};
use axum::body::Bytes;
use serde_json::Value;
use tokio::sync::mpsc;

use super::SubscriptionStream;
use crate::anthropic::subscription::{SubscriptionOptions, run_subscription_model};
use crate::anthropic::subscription_frames::{
    mapped_tool_name, send_text_delta, send_text_finish, send_text_start, send_tool_block,
    send_tool_finish,
};

const BLOCKED_SUBAGENT_NOTICE: &str =
    "The requested SubAgent model is not configured, so it was not started. Continue without it.";

impl SubscriptionStream {
    pub(super) async fn recover_failure(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        error: &anyhow::Error,
    ) -> Result<()> {
        self.activity.close(sender).await?;
        if self.saw_result {
            return Ok(());
        }
        if self.saw_tool_use {
            send_tool_finish(sender, 0).await?;
            self.saw_result = true;
            return Ok(());
        }
        let diagnostic = format!("\n\nClaudex provider stream ended before completion: {error:#}");
        self.append_text(sender, &diagnostic).await?;
        send_text_finish(sender, self.next_index.saturating_sub(1), 0).await?;
        self.text_closed = true;
        self.saw_result = true;
        Ok(())
    }

    pub(super) async fn append_text(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        text: &str,
    ) -> Result<()> {
        self.activity.close(sender).await?;
        if !self.text_started || self.text_closed {
            send_text_start(sender, self.next_index).await?;
            self.text_started = true;
            self.text_closed = false;
            self.next_index += 1;
        }
        send_text_delta(sender, self.next_index.saturating_sub(1), text).await
    }

    pub(super) async fn forward_tool_uses(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        envelope: &Value,
    ) -> Result<()> {
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
        self.saw_tool_use |= forwarded;
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
        if let Some(executor) = self
            .tool_context
            .as_ref()
            .and_then(|context| context.child_executor.clone())
            .filter(|_| super::super::agent_effort::is_agent_tool(name))
        {
            self.execute_child_agent(sender, &executor, &public_input)
                .await?;
            // The adapter consumed this Agent call locally and emitted its
            // result as text. No tool_use block was forwarded to the client.
            return Ok(false);
        }
        send_tool_block(sender, self.next_index, id, name, public_input).await?;
        self.next_index += 1;
        Ok(true)
    }

    async fn execute_child_agent(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        executor: &super::super::subscription::SubscriptionChildExecutor,
        input: &Value,
    ) -> Result<()> {
        let prompt = input
            .get("prompt")
            .and_then(Value::as_str)
            .context("routed SubAgent call has no prompt")?;
        let model = routed_header(prompt, "claudex_model")
            .context("routed SubAgent call has no claudex_model")?;
        let effort = routed_header(prompt, "claudex_effort");
        let options = SubscriptionOptions {
            effort,
            tools: executor.tools.clone(),
            // A direct child owns any further nested Agent calls. They must never
            // escape back to the main session.
            bridge_tools: false,
            cwd: executor.cwd.clone(),
            slots: executor.slots.clone(),
            timeout: executor.timeout,
            tool_context: None,
        };
        let result = run_subscription_model(&executor.program, &model, prompt, options)
            .await
            .with_context(|| format!("direct SubAgent `{model}` execution failed"))?;
        self.append_text(
            sender,
            &format!("Nested SubAgent result ({model}):\n{result}"),
        )
        .await
    }

    async fn report_blocked_subagent(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    ) -> Result<()> {
        self.append_text(sender, BLOCKED_SUBAGENT_NOTICE).await
    }
}

fn routed_header(prompt: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    prompt
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
