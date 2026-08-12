use anyhow::Result;
use axum::body::Bytes;
use serde_json::{Value, json};
use std::convert::Infallible;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::SubscriptionStream;
use crate::anthropic::{
    stream::send_stream_frame, subscription_activity::send_thinking_delta,
    subscription_frames::send_block_stop,
};

impl SubscriptionStream {
    /// Forward Claude subscription stream-json thinking into Claude Code SSE.
    pub(super) async fn forward_thinking_stream_event(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        envelope: &Value,
    ) -> Result<bool> {
        if self.saw_tool_use || self.blocked_subagent || self.saw_result {
            return Ok(false);
        }
        // Never reopen thinking after visible assistant text (block-order safety).
        if self.text_started && !self.text_closed {
            return Ok(false);
        }
        let event = envelope.get("event").unwrap_or(envelope);
        match event.get("type").and_then(Value::as_str) {
            Some("content_block_start")
                if event.pointer("/content_block/type").and_then(Value::as_str)
                    == Some("thinking") =>
            {
                self.open_native_thinking(sender).await
            }
            Some("content_block_delta") | None => self.forward_thinking_delta(sender, event).await,
            Some("content_block_stop") if self.thinking_open.is_some() => {
                self.close_native_thinking(sender).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn forward_thinking_delta(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
        event: &Value,
    ) -> Result<bool> {
        let delta = event.get("delta").unwrap_or(event);
        match delta.get("type").and_then(Value::as_str) {
            Some("thinking_delta") => {
                let thinking = delta
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if thinking.is_empty() {
                    return Ok(false);
                }
                if self.thinking_open.is_none() {
                    let _ = self.open_native_thinking(sender).await?;
                }
                let Some((index, _)) = self.thinking_open else {
                    return Ok(false);
                };
                send_thinking_delta(sender, index, thinking).await?;
                Ok(true)
            }
            Some("signature_delta") => {
                let signature = delta
                    .get("signature")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
                let Some(signature) = signature else {
                    return Ok(false);
                };
                if self.thinking_open.is_none() {
                    let _ = self.open_native_thinking(sender).await?;
                }
                let Some((index, slot)) = self.thinking_open.as_mut() else {
                    return Ok(false);
                };
                *slot = Some(signature.clone());
                send_signature_delta(sender, *index, &signature).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn open_native_thinking(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    ) -> Result<bool> {
        if self.thinking_open.is_some() {
            return Ok(false);
        }
        self.activity.close(sender).await?;
        let index = self.next_index;
        send_stream_frame(Some(sender), "content_block_start", || {
            json!({
                "type":"content_block_start", "index":index,
                "content_block":{"type":"thinking","thinking":"","signature":""}
            })
        })
        .await?;
        self.thinking_open = Some((index, None));
        self.next_index += 1;
        Ok(true)
    }

    pub(super) async fn close_native_thinking(
        &mut self,
        sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    ) -> Result<()> {
        let Some((index, signature)) = self.thinking_open.take() else {
            return Ok(());
        };
        // signature_delta may already have been forwarded; synthesize only if missing.
        if signature.is_none() {
            let signature = format!("claudex_claude_thinking_{}", Uuid::new_v4().simple());
            send_signature_delta(sender, index, &signature).await?;
        }
        send_block_stop(sender, index).await
    }
}

async fn send_signature_delta(
    sender: &mpsc::Sender<Result<Bytes, Infallible>>,
    index: usize,
    signature: &str,
) -> Result<()> {
    send_stream_frame(Some(sender), "content_block_delta", || {
        json!({
            "type":"content_block_delta", "index":index,
            "delta":{"type":"signature_delta","signature":signature}
        })
    })
    .await
}
