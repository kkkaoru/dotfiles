//! Display-only Anthropic tool_use cards for provider-owned tools (Grok ACP).

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{
    builder::SegmentBuilder,
    protocol::{StreamSender, send_stream_frame, send_tool_use},
};

impl SegmentBuilder {
    /// Streams a Claude Code tool card without waiting for Claude tool_result.
    pub(super) async fn provider_tool_call(
        &mut self,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let params = event
            .get("params")
            .context("provider tool call params missing")?;
        let call_id = params
            .get("callId")
            .and_then(Value::as_str)
            .context("provider tool callId missing")?;
        let name = params.get("tool").and_then(Value::as_str).unwrap_or("Tool");
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
        let tool_use_id = format!("toolu_provider_{}", call_id.replace(':', "_"));
        self.close_open_blocks(stream).await?;
        let block = json!({
            "type": "tool_use",
            "id": tool_use_id,
            "name": name,
            "input": arguments,
            "_claudex_provider": true
        });
        let index = self.blocks.len();
        send_tool_use(stream, index, &block).await?;
        self.blocks.push(block);
        self.provider_tool_ids.push(tool_use_id);
        Ok(())
    }

    /// Status / output for a provider tool card already streamed.
    pub(super) async fn provider_tool_update(
        &mut self,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let params = event
            .get("params")
            .context("provider tool update params missing")?;
        let status = params
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("updated");
        let title = params
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("tool");
        match status {
            "failed" => {
                let detail = output_preview(params.get("output"), "failed");
                let preview = truncate_for_status(&detail, 400);
                self.append_text(&format!("\n✗ {title}: {preview}\n"), stream)
                    .await?;
            }
            "completed" => {
                let detail = output_preview(params.get("output"), "");
                if !detail.is_empty() {
                    let preview = truncate_for_status(&detail, 240);
                    self.append_text(&format!("\n✓ {title}: {preview}\n"), stream)
                        .await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) async fn append_text(
        &mut self,
        delta: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if delta.is_empty() {
            return Ok(());
        }
        self.thinking.close(&mut self.blocks, stream).await?;
        let index = match &mut self.open_text_block {
            Some((index, text)) => {
                text.push_str(delta);
                *index
            }
            None => self.start_text_block(delta, stream).await?,
        };
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta", "index":index,
                "delta":{"type":"text_delta","text":delta}
            })
        })
        .await
    }
}

fn output_preview(value: Option<&Value>, fallback: &str) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => fallback.to_owned(),
    }
}

fn truncate_for_status(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let mut chars = trimmed.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
include!("provider_tool_tests.rs");
