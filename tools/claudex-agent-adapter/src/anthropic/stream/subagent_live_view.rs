//! Test helper: what a Claude Code SubAgent panel can paint *during* a turn.
//!
//! SubAgent TUI hides `text_delta` until `end_turn`. Closing thinking mid-turn
//! collapses Claude Code 2.1 to "Thought for Xs". ACP progress therefore stays
//! on one native thinking block (▶/✓) before `finish` / `message_stop`. Command
//! Code still uses display-only `server_tool_use`; Codex and Pi/Grok paint via
//! native `tool_use` cards. ACP workers keep ▶ chrome on thinking.

use axum::body::Bytes;
use serde_json::Value;
use std::convert::Infallible;
use tokio::sync::mpsc;

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct SubAgentLiveView {
    /// Thinking chrome visible while the SubAgent is still running.
    pub visible_thinking: String,
    /// Assistant text accumulated but hidden until end_turn.
    pub hidden_text: String,
    /// Display-only `server_tool_use` cards painted mid-turn (web_search / web_fetch).
    pub visible_server_tools: Vec<String>,
    /// Native Anthropic `tool_use` cards painted mid-turn (Pi/Grok Read/Bash).
    pub visible_tool_use: Vec<String>,
    pub saw_end_turn: bool,
    pub saw_message_stop: bool,
}

impl SubAgentLiveView {
    pub fn ingest_sse(&mut self, sse: &str) {
        for payload in sse_json_payloads(sse) {
            self.ingest_payload(&payload);
        }
    }

    fn ingest_payload(&mut self, payload: &Value) {
        match payload.get("type").and_then(Value::as_str) {
            Some("content_block_start") => self.ingest_block_start(payload),
            Some("content_block_delta") => self.ingest_block_delta(payload),
            Some("message_delta") => self.ingest_message_delta(payload),
            Some("message_stop") => self.saw_message_stop = true,
            _ => {}
        }
    }

    fn ingest_block_start(&mut self, payload: &Value) {
        let Some(name) = payload["content_block"]["name"].as_str() else {
            return;
        };
        match payload["content_block"]["type"].as_str() {
            Some("server_tool_use") => self.visible_server_tools.push(name.to_owned()),
            Some("tool_use") => self.visible_tool_use.push(name.to_owned()),
            _ => {}
        }
    }

    fn ingest_block_delta(&mut self, payload: &Value) {
        let Some(kind) = payload["delta"]["type"].as_str() else {
            return;
        };
        match kind {
            "thinking_delta" => append_str_field(&mut self.visible_thinking, payload, "thinking"),
            "text_delta" => append_str_field(&mut self.hidden_text, payload, "text"),
            _ => {}
        }
    }

    fn ingest_message_delta(&mut self, payload: &Value) {
        if payload["delta"]["stop_reason"].as_str() == Some("end_turn") {
            self.saw_end_turn = true;
        }
    }

    /// Drain frames already queued without waiting for the sender to close.
    /// Call after each ACP event to assert mid-turn display.
    pub fn ingest_available(&mut self, receiver: &mut mpsc::Receiver<Result<Bytes, Infallible>>) {
        let mut sse = String::new();
        while let Ok(frame) = receiver.try_recv() {
            sse.push_str(&String::from_utf8_lossy(
                &frame.expect("infallible SSE frame"),
            ));
        }
        self.ingest_sse(&sse);
    }

    pub fn turn_still_open(&self) -> bool {
        !self.saw_end_turn && !self.saw_message_stop
    }
}

pub(super) fn sse_json_payloads(sse: &str) -> Vec<Value> {
    sse.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect()
}

fn append_str_field(target: &mut String, payload: &Value, field: &str) {
    if let Some(delta) = payload["delta"][field].as_str() {
        target.push_str(delta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_view_treats_thinking_as_visible_and_text_as_hidden_before_end_turn() {
        let mut view = SubAgentLiveView::default();
        view.ingest_sse(
            "event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Phase 1.\\n\"}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"\\n▶ ReadFile\\n\"}}\n\n",
        );
        assert!(view.turn_still_open());
        assert_eq!(view.hidden_text, "Phase 1.\n");
        assert_eq!(view.visible_thinking, "\n▶ ReadFile\n");
        assert!(view.visible_server_tools.is_empty());
        assert!(view.visible_tool_use.is_empty());
        view.ingest_sse(
            "event: content_block_start\n\
             data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvtoolu_1\",\"name\":\"web_search\",\"input\":{}}}\n\n",
        );
        assert_eq!(view.visible_server_tools, vec!["web_search".to_owned()]);
        view.ingest_sse(
            "event: content_block_start\n\
             data: {\"type\":\"content_block_start\",\"index\":3,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"Read\",\"input\":{}}}\n\n",
        );
        assert_eq!(view.visible_tool_use, vec!["Read".to_owned()]);
        view.ingest_sse(
            "event: message_delta\n\
             data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null}}\n\n\
             event: message_stop\n\
             data: {\"type\":\"message_stop\"}\n\n",
        );
        assert!(!view.turn_still_open());
        assert!(view.saw_end_turn);
        assert!(view.saw_message_stop);
    }

    #[test]
    fn live_view_skips_incomplete_payload_fields() {
        let mut view = SubAgentLiveView::default();
        view.ingest_sse(
            "event: content_block_start\n\
             data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"1\",\"input\":{}}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"thinking_delta\"}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\"}}\n\n\
             event: message_delta\n\
             data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"signature_delta\",\"signature\":\"x\"}}\n\n",
        );
        assert!(view.visible_server_tools.is_empty());
        assert!(view.visible_thinking.is_empty());
        assert!(view.hidden_text.is_empty());
        assert!(!view.saw_end_turn);
        assert!(view.turn_still_open());
    }
}
