use anyhow::Result;
use axum::body::Bytes;
use serde_json::Value;
use std::convert::Infallible;
use tokio::sync::mpsc;

use super::SubscriptionStream;
use crate::anthropic::{
    subscription::SubscriptionOptions,
    subscription_frames::{send_block_stop, send_text_delta, send_text_start},
};

impl SubscriptionStream {
    pub(super) async fn handle_envelope(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        envelope: &Value,
    ) -> Result<bool> {
        match envelope.get("type").and_then(Value::as_str) {
            Some("stream_event") => self.forward_text_delta(sender, envelope).await,
            Some("assistant") => self.forward_tool_uses(sender, envelope).await,
            Some("result") => {
                self.finish(sender, envelope).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(super) async fn forward_text_delta(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        envelope: &Value,
    ) -> Result<bool> {
        if envelope
            .pointer("/event/delta/type")
            .and_then(Value::as_str)
            != Some("text_delta")
        {
            return Ok(false);
        }
        let text = envelope
            .pointer("/event/delta/text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if text.is_empty() {
            return Ok(false);
        }
        if self.saw_tool_use || self.blocked_subagent {
            return Ok(false);
        }
        self.activity.close(sender).await?;
        if !self.text_started || self.text_closed {
            send_text_start(sender, self.next_index).await?;
            self.text_started = true;
            self.text_closed = false;
            self.next_index += 1;
        }
        send_text_delta(sender, self.next_index.saturating_sub(1), text).await?;
        Ok(true)
    }


    pub(super) async fn close_text(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    ) -> Result<()> {
        if self.text_started && !self.text_closed {
            send_block_stop(sender, self.next_index.saturating_sub(1)).await?;
            self.text_closed = true;
        }
        Ok(())
    }

    pub(super) async fn start_subagent_activity(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        options: &SubscriptionOptions,
        model: &str,
    ) -> Result<()> {
        if !options.is_subagent {
            return Ok(());
        }
        // ZWSP-only prime matches ACP SubAgents: prose like "SubAgent starting"
        // collapses Claude Code 2.1 into Wandering until the first tool_use.
        let _ = model;
        self.activity
            .start_status(sender, "\u{200b}", &mut self.next_index)
            .await
    }

    pub(super) async fn activity_keepalive(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    ) -> Result<()> {
        if self.saw_result || self.saw_tool_use || self.blocked_subagent {
            return Ok(());
        }
        if self.text_closed {
            send_text_start(sender, self.next_index).await?;
            self.text_started = true;
            self.text_closed = false;
            self.next_index += 1;
        }
        let text_index = self.text_started.then(|| self.next_index.saturating_sub(1));
        self.activity
            .keepalive(sender, text_index, &mut self.next_index)
            .await
    }
}
