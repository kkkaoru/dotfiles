//! Compact live progress for provider-owned tools (Grok / Cursor / configured ACP).
//!
//! Never emitted as Anthropic `tool_use` (Claude Code would re-execute). ACP
//! SubAgents, including Command Code, keep native thinking open with ▶/✓
//! markers for the whole turn. Only short status markers are committed — full
//! tool payloads freeze the TUI when every Bash/Read result is dumped into
//! assistant text.

use anyhow::{Context, Result};
use serde_json::Value;

use super::{builder::SegmentBuilder, protocol::StreamSender};

mod preview;
mod terminal;
#[cfg(test)]
#[allow(unused_imports)]
use preview::{
    argument_preview, failure_preview, first_line, object_failure_preview, scalar_preview,
    truncate_for_status, valid_source_url, valid_source_urls, validated_provider_web_evidence,
};
use preview::{compact_title, progress_start_line};

impl SegmentBuilder {
    /// Streams provider-owned work as thinking chrome, never as Anthropic `tool_use`.
    ///
    /// Claude Code executes every `tool_use` block it receives, even when the
    /// provider already executed that tool and the message ends with `end_turn`.
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
        let is_new = self.remember_provider_tool(call_id, title);
        self.record_provider_web_evidence(call_id, params);
        if !is_new {
            return Ok(());
        }
        self.stream_progress_text(&progress_start_line(title, params.get("arguments")), stream)
            .await
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
        if let Some(call_id) = call_id {
            self.record_provider_web_evidence(call_id, params);
        }
        let short_title = compact_title(&title);
        match status {
            "failed" => {
                self.finish_provider_tool_terminal(call_id, &short_title, params, false, stream)
                    .await?;
            }
            // Success: marker only. Dumping stdout/JSON here flooded the TUI and
            // made long Grok/Cursor turns look frozen on a wall of tool logs.
            "completed" => {
                self.finish_provider_tool_terminal(call_id, &short_title, params, true, stream)
                    .await?;
            }
            "pending" | "in_progress" => {
                self.start_provider_tool_from_update(call_id, &title, params, stream)
                    .await?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "provider_tool_tests.rs"]
mod tests;
