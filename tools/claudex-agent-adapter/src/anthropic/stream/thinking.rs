use anyhow::Result;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{StreamSender, send_stream_frame};

const HEARTBEAT: &str = "\u{200b}";

#[derive(Default)]
pub(super) struct ThinkingState {
    open: Option<OpenThinking>,
}

struct OpenThinking {
    index: usize,
    item_id: String,
    summary_index: i64,
    signature: String,
    text: String,
}

impl ThinkingState {
    pub(super) fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub(super) fn is_native_thought_open(&self) -> bool {
        self.open.as_ref().is_some_and(|open| {
            open.item_id != "claudex_provider_progress"
                && open.item_id != "claudex_activity_keepalive"
        })
    }

    pub(super) async fn delta(
        &mut self,
        event: &Value,
        blocks: &mut Vec<Value>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some((item_id, summary_index, delta)) = summary_delta(event) else {
            return Ok(());
        };
        self.delta_text(item_id, summary_index, delta, blocks, stream)
            .await
    }

    pub(super) async fn delta_text(
        &mut self,
        item_id: &str,
        summary_index: i64,
        delta: &str,
        blocks: &mut Vec<Value>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.delta_text_on(item_id, summary_index, delta, false, blocks, stream)
            .await
    }

    /// SubAgent turns keep one native thinking block open so Claude Code's
    /// standard thinking chrome stays live. Closing on every ACP itemId /
    /// summaryIndex collapse the viewer to repeating "Thought for Xs".
    pub(super) async fn delta_text_coalesced(
        &mut self,
        item_id: &str,
        summary_index: i64,
        delta: &str,
        blocks: &mut Vec<Value>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.delta_text_on(item_id, summary_index, delta, true, blocks, stream)
            .await
    }

    async fn delta_text_on(
        &mut self,
        item_id: &str,
        summary_index: i64,
        delta: &str,
        coalesce: bool,
        blocks: &mut Vec<Value>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if delta.trim().is_empty() || has_visible_output(blocks) {
            return Ok(());
        }
        // Main session: one Anthropic thinking block per (itemId, summaryIndex).
        // SubAgents coalesce so thinking stays synced for the whole turn.
        let unit_changed = !coalesce
            && self
                .open
                .as_ref()
                .is_some_and(|open| open.item_id != item_id || open.summary_index != summary_index);
        if unit_changed {
            self.close(blocks, stream).await?;
        }
        if self.open.is_none() {
            self.start(blocks, item_id, summary_index, stream).await?;
        } else if coalesce
            && self.open.as_ref().is_some_and(|open| {
                open.item_id == "claudex_provider_progress"
                    || open.item_id == "claudex_activity_keepalive"
            })
        {
            // Tool/prose may open progress chrome first. Promote it to native
            // thought so sanitize keeps the reasoning in the transcript.
            if let Some(open) = self.open.as_mut() {
                open.item_id = item_id.to_owned();
                open.signature = thinking_signature(item_id);
            }
        }
        let open = self.open.as_mut().expect("thinking block just opened");
        open.summary_index = summary_index;
        open.text.push_str(delta);
        blocks[open.index]["thinking"] = json!(open.text);
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta", "index":open.index,
                "delta":{"type":"thinking_delta","thinking":delta}
            })
        })
        .await
    }

    async fn start(
        &mut self,
        blocks: &mut Vec<Value>,
        item_id: &str,
        summary_index: i64,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let index = blocks.len();
        blocks.push(json!({"type":"thinking","thinking":"","signature":""}));
        send_stream_frame(stream, "content_block_start", || {
            json!({
                "type":"content_block_start", "index":index,
                "content_block":{"type":"thinking","thinking":"","signature":""}
            })
        })
        .await?;
        self.open = Some(OpenThinking {
            index,
            item_id: item_id.to_owned(),
            summary_index,
            signature: thinking_signature(item_id),
            text: String::new(),
        });
        Ok(())
    }

    pub(super) async fn close(
        &mut self,
        blocks: &mut [Value],
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let Some(open) = self.open.take() else {
            return Ok(());
        };
        blocks[open.index]["thinking"] = json!(open.text);
        blocks[open.index]["signature"] = json!(open.signature);
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta", "index":open.index,
                "delta":{"type":"signature_delta","signature":blocks[open.index]["signature"]}
            })
        })
        .await?;
        send_stream_frame(
            stream,
            "content_block_stop",
            || json!({"type":"content_block_stop","index":open.index}),
        )
        .await
    }

    /// Append provider tool progress to the open thinking chrome.
    ///
    /// Unlike [`activity_status`], this keeps emitting ▶/✓/✗ lines for each ACP
    /// tool. Claude Code SubAgent panels show thinking live; assistant text is
    /// often hidden until end_turn and then stripped by sanitize.
    pub(super) async fn progress_status(
        &mut self,
        blocks: &mut Vec<Value>,
        status: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if status.is_empty() {
            return Ok(());
        }
        self.progress_status_on(blocks, status, false, stream).await
    }

    /// Append ▶/✓/elapsed into the open thinking block without closing it.
    /// Used for ACP SubAgents so Claude Code's native thinking chrome stays live.
    pub(super) async fn progress_status_keep_open(
        &mut self,
        blocks: &mut Vec<Value>,
        status: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if status.is_empty() {
            return Ok(());
        }
        self.progress_status_on(blocks, status, true, stream).await
    }

    async fn progress_status_on(
        &mut self,
        blocks: &mut Vec<Value>,
        status: &str,
        keep_open: bool,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        // Cline/DeepSeek often open a blank or long CoT thinking unit first.
        // Closing it to switch chrome left SubAgent TUI on "Thought for Xs".
        if !keep_open
            && self
                .open
                .as_ref()
                .is_some_and(|open| open.item_id != "claudex_provider_progress")
        {
            self.close(blocks, stream).await?;
        }
        if self.open.is_none() {
            self.start(blocks, "claudex_provider_progress", 0, stream)
                .await?;
        }
        let open = self.open.as_mut().expect("progress block just opened");
        open.text.push_str(status);
        blocks[open.index]["thinking"] = json!(open.text);
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta", "index":open.index,
                "delta":{"type":"thinking_delta","thinking":status}
            })
        })
        .await
    }

    /// Emit a decoded content event so Claude Code's SubAgent stream watchdog
    /// (~600s decoded-event idle) does not fire during long provider-side tool
    /// waits. The first heartbeat is visible so a silent provider does not leave
    /// the user with only a spinner; later heartbeats stay zero-width to avoid
    /// noisy repetition.
    ///
    /// Anthropic `ping` frames keep the raw-byte idle timer alive (~180s) but
    /// do not reset the decoded-event timer. Pure keepalive thinking is stripped
    /// from the committed segment so transcripts stay clean.
    pub(super) async fn activity_status(
        &mut self,
        blocks: &mut Vec<Value>,
        status: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if status.is_empty() || has_visible_output(blocks) {
            return Ok(());
        }
        if self.open.is_none() {
            self.start(blocks, "claudex_activity_keepalive", 0, stream)
                .await?;
        }
        let open = self.open.as_mut().expect("activity block just opened");
        if open.item_id != "claudex_activity_keepalive" || !open.text.is_empty() {
            return Ok(());
        }
        open.text.push_str(status);
        blocks[open.index]["thinking"] = json!(open.text);
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta", "index":open.index,
                "delta":{"type":"thinking_delta","thinking":status}
            })
        })
        .await
    }

    pub(super) fn prime_silent_heartbeat(&mut self, blocks: &mut Vec<Value>) {
        if self.open.is_some() {
            return;
        }
        let index = blocks.len();
        blocks.push(json!({
            "type":"thinking",
            "thinking": HEARTBEAT,
            "signature":""
        }));
        self.open = Some(OpenThinking {
            index,
            item_id: "claudex_activity_keepalive".to_owned(),
            summary_index: 0,
            signature: thinking_signature("claudex_activity_keepalive"),
            text: HEARTBEAT.to_owned(),
        });
    }

    pub(super) async fn activity_keepalive(
        &mut self,
        blocks: &mut Vec<Value>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.emit_activity_heartbeat(blocks, stream).await
    }

    async fn emit_activity_heartbeat(
        &mut self,
        blocks: &mut Vec<Value>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        // Keep emitting decoded deltas after text/tool_use. Spark/Codex
        // SubAgents often go silent during native tools once a Bash/WebSearch
        // block exists; stopping heartbeats here used to trip Claude Code's
        // 600s "stream watchdog did not recover".
        const STATUS: &str = "Claudex is still working; waiting for provider output\u{2026}";
        if self.open.as_ref().is_some_and(|open| {
            open.item_id != "claudex_activity_keepalive"
                && open.item_id != "claudex_provider_progress"
                && open.text.trim().is_empty()
        }) {
            self.close(blocks, stream).await?;
        }
        if self.open.is_none() {
            self.start(blocks, "claudex_activity_keepalive", 0, stream)
                .await?;
        }
        let open = self.open.as_mut().expect("thinking block just opened");
        let delta = if open.item_id == "claudex_activity_keepalive" && open.text.is_empty() {
            STATUS
        } else {
            HEARTBEAT
        };
        open.text.push_str(delta);
        blocks[open.index]["thinking"] = json!(open.text);
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta",
                "index":open.index,
                "delta":{"type":"thinking_delta","thinking":delta}
            })
        })
        .await
    }

    /// Visible elapsed tick so a silent Cline/Qwen SubAgent does not sit on an
    /// empty viewer for minutes. Zero-width heartbeats keep the watchdog alive
    /// but do not paint Claude Code's SubAgent panel.
    pub(super) async fn elapsed_keepalive(
        &mut self,
        blocks: &mut Vec<Value>,
        elapsed: std::time::Duration,
        last_tool: Option<&str>,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        if self.is_native_thought_open() {
            // Live native thinking already drives Claude Code's elapsed chrome.
            return Ok(());
        }
        let secs = elapsed.as_secs().max(1);
        let label = if secs < 60 {
            format!("{secs}s")
        } else {
            format!("{}m", secs / 60)
        };
        let tool = last_tool
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(|title| format!(" · last: {title}"))
            .unwrap_or_default();
        self.progress_status_keep_open(
            blocks,
            &format!("\n… still working ({label}){tool}\n"),
            stream,
        )
        .await
    }
}

fn thinking_signature(item_id: &str) -> String {
    match item_id {
        "claudex_provider_progress" | "claudex_activity_keepalive" => item_id.to_owned(),
        _ => format!("claudex_local_{}", Uuid::new_v4().simple()),
    }
}

pub(super) fn summary_delta(event: &Value) -> Option<(&str, i64, &str)> {
    let params = event.get("params")?;
    Some((
        params.get("itemId")?.as_str()?,
        params.get("summaryIndex")?.as_i64()?,
        params.get("delta")?.as_str()?,
    ))
}

pub(super) fn has_visible_output(blocks: &[Value]) -> bool {
    blocks.iter().any(|block| {
        !matches!(
            block.get("type").and_then(Value::as_str),
            Some("thinking") | Some("server_tool_use")
        )
    })
}
