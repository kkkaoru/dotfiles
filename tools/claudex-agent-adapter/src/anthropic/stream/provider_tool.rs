//! Display-only progress for provider-owned tools (Grok ACP).

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{
    builder::SegmentBuilder,
    protocol::{StreamSender, send_stream_frame},
};

const FAILED_STATUS_PREVIEW_CHAR_LIMIT: usize = 400;
const COMPLETED_STATUS_PREVIEW_CHAR_LIMIT: usize = 240;

impl SegmentBuilder {
    /// Streams provider-owned work as text, never as Anthropic `tool_use`.
    ///
    /// Claude Code executes every `tool_use` block it receives, even when the
    /// provider already executed that tool and the message ends with `end_turn`.
    /// A display-only card would therefore produce a synthetic
    /// `No such tool available` result and an unnecessary follow-up turn.
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
        let title = params
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(name);
        if !self.remember_provider_tool(call_id, title) {
            return Ok(());
        }
        self.append_text(&format!("\n\n▶ {title}\n"), stream).await
    }

    /// Status / output for provider-owned work already streamed.
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
        let call_id = params.get("callId").and_then(Value::as_str);
        let explicit_title = params
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty());
        let title = explicit_title
            .or_else(|| call_id.and_then(|id| self.provider_tool_title(id)))
            .unwrap_or("tool")
            .to_owned();
        match status {
            "failed" => {
                let detail = output_preview(params.get("output"), "failed");
                let preview = truncate_for_status(&detail, FAILED_STATUS_PREVIEW_CHAR_LIMIT);
                self.append_text(&format!("\n✗ {title}: {preview}\n"), stream)
                    .await?;
            }
            "completed" => {
                let detail = output_preview(params.get("output"), "");
                if !detail.is_empty() {
                    let preview = truncate_for_status(&detail, COMPLETED_STATUS_PREVIEW_CHAR_LIMIT);
                    self.append_text(&format!("\n✓ {title}: {preview}\n"), stream)
                        .await?;
                }
            }
            "pending" | "in_progress" => {
                self.start_provider_tool_from_update(call_id, &title, stream)
                    .await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn start_provider_tool_from_update(
        &mut self,
        call_id: Option<&str>,
        title: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some(call_id) = call_id else {
            return Ok(());
        };
        if !self.remember_provider_tool(call_id, title) {
            return Ok(());
        }
        self.append_text(&format!("\n\n▶ {title}\n"), stream).await
    }

    fn remember_provider_tool(&mut self, call_id: &str, title: &str) -> bool {
        if self.provider_tool_title(call_id).is_some() {
            return false;
        }
        self.provider_tool_calls
            .push((call_id.to_owned(), title.to_owned()));
        true
    }

    fn provider_tool_title(&self, call_id: &str) -> Option<&str> {
        self.provider_tool_calls
            .iter()
            .find(|(seen, _)| seen == call_id)
            .map(|(_, title)| title.as_str())
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
