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

/// Failures keep a brief hint; successes never attach tool bodies.
const FAILED_STATUS_PREVIEW_CHAR_LIMIT: usize = 80;
const TITLE_CHAR_LIMIT: usize = 48;
const ARG_PREVIEW_CHAR_LIMIT: usize = 48;

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
                self.emit_terminal_failure(call_id, &short_title, params, stream)
                    .await?;
            }
            // Success: marker only. Dumping stdout/JSON here flooded the TUI and
            // made long Grok/Cursor turns look frozen on a wall of tool logs.
            "completed" => {
                self.emit_terminal_success(call_id, &short_title, stream)
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

    async fn emit_terminal_failure(
        &mut self,
        call_id: Option<&str>,
        short_title: &str,
        params: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if terminal_already_emitted(&mut self.provider_tool_terminal_ids, call_id) {
            return Ok(());
        }
        let detail = failure_preview(params.get("output"));
        let preview = truncate_for_status(&detail, FAILED_STATUS_PREVIEW_CHAR_LIMIT);
        self.stream_progress_text(&format!("\n✗ {short_title}: {preview}\n"), stream)
            .await
    }

    async fn emit_terminal_success(
        &mut self,
        call_id: Option<&str>,
        short_title: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if terminal_already_emitted(&mut self.provider_tool_terminal_ids, call_id) {
            return Ok(());
        }
        self.stream_progress_text(&format!("\n✓ {short_title}\n"), stream)
            .await
    }

    async fn start_provider_tool_from_update(
        &mut self,
        call_id: Option<&str>,
        title: &str,
        params: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some(call_id) = call_id else {
            return Ok(());
        };
        if !self.remember_provider_tool(call_id, title) {
            return Ok(());
        }
        self.stream_progress_text(&progress_start_line(title, params.get("arguments")), stream)
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
        && source_urls
            .map(Vec::as_slice)
            .is_some_and(valid_source_urls)
}

fn valid_source_urls(urls: &[Value]) -> bool {
    !urls.is_empty() && urls.iter().all(valid_source_url)
}

fn valid_source_url(url: &Value) -> bool {
    url.as_str()
        .is_some_and(|url| url.starts_with("https://") || url.starts_with("http://"))
}

fn progress_start_line(title: &str, arguments: Option<&Value>) -> String {
    let short_title = compact_title(title);
    match argument_preview(arguments) {
        // Prefer a one-line command/path snippet; never dump full argument JSON.
        Some(detail) if !short_title.contains(detail.as_str()) => {
            format!("\n▶ {short_title}: {detail}\n")
        }
        _ => format!("\n▶ {short_title}\n"),
    }
}

fn compact_title(title: &str) -> String {
    let trimmed = title.trim();
    // Provider titles sometimes embed the whole command; keep the tool name only.
    let head = trimmed
        .split_once(':')
        .map(|(name, _)| name.trim())
        .filter(|name| !name.is_empty() && name.len() <= TITLE_CHAR_LIMIT)
        .unwrap_or(trimmed);
    truncate_for_status(head, TITLE_CHAR_LIMIT)
}

fn argument_preview(arguments: Option<&Value>) -> Option<String> {
    let Value::Object(map) = arguments? else {
        return None;
    };
    for key in [
        "command",
        "cmd",
        "path",
        "file_path",
        "target_file",
        "query",
        "pattern",
        "url",
        "description",
    ] {
        if let Some(value) = map.get(key).and_then(scalar_preview) {
            return Some(truncate_for_status(
                &first_line(&value),
                ARG_PREVIEW_CHAR_LIMIT,
            ));
        }
    }
    None
}

fn scalar_preview(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn failure_preview(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => first_line(text),
        Some(Value::Object(map)) => object_failure_preview(map),
        Some(_) | None => "failed".to_owned(),
    }
}

fn object_failure_preview(map: &serde_json::Map<String, Value>) -> String {
    for key in ["error", "message", "stderr", "stdout", "reason"] {
        let Some(text) = map.get(key).and_then(Value::as_str) else {
            continue;
        };
        let line = first_line(text);
        if !line.is_empty() {
            return line;
        }
    }
    "failed".to_owned()
}

fn terminal_already_emitted(ids: &mut std::collections::HashSet<String>, call_id: Option<&str>) -> bool {
    call_id.is_some_and(|call_id| !ids.insert(call_id.to_owned()))
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_owned()
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
