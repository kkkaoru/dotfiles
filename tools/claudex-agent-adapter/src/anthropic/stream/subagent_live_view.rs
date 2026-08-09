//! Test helper: what a Claude Code SubAgent panel can paint *during* a turn.
//!
//! SubAgent TUI shows `thinking_delta` live and often hides `text_delta` until
//! `end_turn`. Cline/Qwen/Cursor progress must therefore arrive as thinking
//! frames before `finish` / `message_stop`. Command Code is the opposite:
//! Claude Code 2.1 collapses open thinking into Doing/Orbiting, so ●/▶ and
//! answers stream as live `text_delta` instead.

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
    pub saw_end_turn: bool,
    pub saw_message_stop: bool,
}

impl SubAgentLiveView {
    pub fn ingest_sse(&mut self, sse: &str) {
        for payload in sse_json_payloads(sse) {
            match payload.get("type").and_then(Value::as_str) {
                Some("content_block_delta") => match payload["delta"]["type"].as_str() {
                    Some("thinking_delta") => {
                        if let Some(delta) = payload["delta"]["thinking"].as_str() {
                            self.visible_thinking.push_str(delta);
                        }
                    }
                    Some("text_delta") => {
                        if let Some(delta) = payload["delta"]["text"].as_str() {
                            self.hidden_text.push_str(delta);
                        }
                    }
                    _ => {}
                },
                Some("message_delta") => {
                    if payload["delta"]["stop_reason"].as_str() == Some("end_turn") {
                        self.saw_end_turn = true;
                    }
                }
                Some("message_stop") => self.saw_message_stop = true,
                _ => {}
            }
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
}
