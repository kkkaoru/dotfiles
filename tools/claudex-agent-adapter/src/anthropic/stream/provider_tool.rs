//! Ephemeral WIP progress for provider-owned tools (Grok / configured ACP).
//!
//! Progress is streamed for live visibility but not committed as answer text.

use anyhow::{Context, Result};
use serde_json::Value;

use super::{builder::SegmentBuilder, protocol::StreamSender};

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
        let is_new = self.remember_provider_tool(call_id, title);
        self.record_provider_web_evidence(call_id, params);
        if !is_new {
            return Ok(());
        }
        self.stream_ephemeral_status(&format!("\n\n▶ {title}\n"), stream)
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
        match status {
            "failed" => {
                let detail = output_preview(params.get("output"), "failed");
                let preview = truncate_for_status(&detail, FAILED_STATUS_PREVIEW_CHAR_LIMIT);
                self.stream_ephemeral_status(&format!("\n✗ {title}: {preview}\n"), stream)
                    .await?;
            }
            "completed" => {
                let detail = output_preview(params.get("output"), "");
                if !detail.is_empty() {
                    let preview = truncate_for_status(&detail, COMPLETED_STATUS_PREVIEW_CHAR_LIMIT);
                    self.stream_ephemeral_status(&format!("\n✓ {title}: {preview}\n"), stream)
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
        self.stream_ephemeral_status(&format!("\n\n▶ {title}\n"), stream)
            .await
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

    fn record_provider_web_evidence(&mut self, call_id: &str, params: &Value) {
        if !validated_provider_web_evidence(params.get("evidence")) {
            return;
        }
        self.record_verified_web_evidence(call_id);
    }
}

fn validated_provider_web_evidence(evidence: Option<&Value>) -> bool {
    let Some(evidence) = evidence else {
        return false;
    };
    let source_urls = evidence.get("source_urls").and_then(Value::as_array);
    evidence.get("provider").and_then(Value::as_str) == Some("acp")
        && evidence.get("provenance").and_then(Value::as_str)
            == Some("provider-native-tool-completion")
        && matches!(
            (
                evidence.get("kind").and_then(Value::as_str),
                evidence.get("evidence_class").and_then(Value::as_str),
            ),
            (Some("web_search"), Some("search_result_only"))
                | (Some("web_fetch"), Some("fetch_verified"))
        )
        && evidence.get("status").and_then(Value::as_str) == Some("completed")
        && evidence.get("verified").and_then(Value::as_bool) == Some(true)
        && evidence
            .get("result_summary")
            .and_then(Value::as_str)
            .is_some_and(|summary| !summary.trim().is_empty())
        && source_urls.is_some_and(|urls| {
            !urls.is_empty()
                && urls.iter().all(|url| {
                    url.as_str().is_some_and(|url| {
                        url.starts_with("https://") || url.starts_with("http://")
                    })
                })
        })
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
