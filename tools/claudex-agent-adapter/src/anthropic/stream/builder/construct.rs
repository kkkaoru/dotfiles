use std::{collections::HashSet, time::Instant};

use super::super::sanitize::sanitize_committed_blocks;
use super::super::thinking::ThinkingState;
use super::{SSE_INDEX_PAD, SegmentBuilder};
use crate::anthropic::Usage;
use serde_json::json;

impl SegmentBuilder {
    pub(in crate::anthropic::stream) fn new(input_tokens: u64) -> Self {
        Self {
            blocks: Vec::new(),
            thinking: ThinkingState::default(),
            open_text_block: None,
            pending_answer: String::new(),
            pending_reasoning: String::new(),
            is_subagent: false,
            paint_command_code_progress: false,
            turn_started_at: Instant::now(),
            last_visible_provider_at: Instant::now(),
            last_pi_reasoning_status_at: None,
            pi_reasoning_updates: 0,
            pi_reasoning_chars: 0,
            external_tool_calls: 0,
            provider_tool_calls: Vec::new(),
            provider_tool_terminal_ids: HashSet::new(),
            saw_provider_turn_activity: false,
            bridged_provider_launch_ids: Vec::new(),
            mcp_provider_call_ids: Vec::new(),
            incomplete_launch_call_ids: Vec::new(),
            dropped_launch_call_ids: Vec::new(),
            bulk_dump_hinted: false,
            requires_verified_web_evidence: false,
            verified_web_evidence_call_ids: Vec::new(),
            summarized_reasoning_ids: Vec::new(),
            injected_output_tokens: 0,
            provider_stop_reason: None,
            suppressed_tool_use: false,
            recoverable_empty_output: false,
            requires_subagent_launch: false,
            usage: Usage {
                input_tokens,
                ..Usage::default()
            },
            last_turn_progress: Vec::new(),
            streaming_tool: None,
            consecutive_invalid_tool_json: 0,
        }
    }

    pub(in crate::anthropic::stream) fn with_subagent(mut self, is_subagent: bool) -> Self {
        self.is_subagent = is_subagent;
        self
    }

    pub(in crate::anthropic::stream) fn with_command_code_progress(
        mut self,
        enabled: bool,
    ) -> Self {
        self.paint_command_code_progress = enabled;
        self
    }

    #[cfg(test)]
    pub(in crate::anthropic::stream) fn age_turn_for_test(&mut self, age: std::time::Duration) {
        self.turn_started_at = Instant::now() - age;
    }

    #[cfg(test)]
    pub(in crate::anthropic::stream) fn age_pi_reasoning_status_for_test(
        &mut self,
        age: std::time::Duration,
    ) {
        self.last_pi_reasoning_status_at = Some(Instant::now() - age);
    }

    pub(in crate::anthropic::stream) fn for_turn(
        input_tokens: u64,
        is_subagent: bool,
        model: &str,
    ) -> Self {
        Self::new(input_tokens)
            .with_subagent(is_subagent)
            .with_command_code_progress(crate::anthropic::is_command_code_model(model))
    }

    pub(in crate::anthropic::stream) fn with_reserved_sse_slots(mut self, count: usize) -> Self {
        self.blocks
            .extend(std::iter::repeat_n(json!({"type": SSE_INDEX_PAD}), count));
        self
    }

    pub(in crate::anthropic::stream) fn is_command_code_subagent(&self) -> bool {
        self.is_subagent && self.paint_command_code_progress
    }

    pub(in crate::anthropic::stream) fn with_primed_thinking(mut self) -> Self {
        // Kept as a compatibility builder hook for callers/tests. A prime must
        // not reserve an empty thinking block before real provider output.
        self.thinking.prime_silent_heartbeat(&mut self.blocks);
        self
    }

    pub(in crate::anthropic::stream) fn has_external_tool_calls(&self) -> bool {
        self.external_tool_calls > 0
    }

    /// True once the SubAgent has painted provider ▶ tools, bridged Claude
    /// tool_use, or other provider turn output (Status / answer chrome). Early
    /// Claude Code SSE drops (message_start only) must keep ACP alive; a later
    /// drop after live work is treated as user stop/interrupt.
    pub(in crate::anthropic::stream) fn has_live_provider_work(&self) -> bool {
        self.external_tool_calls > 0
            || self.streaming_tool.is_some()
            || !self.provider_tool_calls.is_empty()
            || self.saw_provider_turn_activity
    }

    pub(in crate::anthropic::stream::builder) fn note_provider_turn_activity(&mut self) {
        self.saw_provider_turn_activity = true;
        self.note_visible_provider_activity();
    }

    pub(in crate::anthropic::stream) fn note_visible_provider_activity(&mut self) {
        self.last_visible_provider_at = Instant::now();
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(in crate::anthropic::stream) fn backdate_last_visible_provider_activity(
        &mut self,
        elapsed: std::time::Duration,
    ) {
        self.last_visible_provider_at = Instant::now()
            .checked_sub(elapsed)
            .expect("test backdate must fit within Instant range");
    }

    /// Activity-based SubAgent bound: synthetic keepalives do not refresh this.
    pub(in crate::anthropic::stream) fn subagent_provider_silence_exceeded(
        &self,
        judgment: std::time::Duration,
    ) -> bool {
        self.is_subagent
            && self
                .last_visible_provider_at
                .elapsed()
                .checked_sub(judgment)
                .is_some()
    }

    pub(in crate::anthropic::stream) fn snapshot_turn_progress(
        &self,
    ) -> Vec<crate::anthropic::TurnProgressEvent> {
        let elapsed_ms =
            u64::try_from(self.turn_started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.provider_tool_calls
            .iter()
            .map(|(id, title)| self.progress_event(id, title, elapsed_ms))
            .chain(self.dropped_launch_call_ids.iter().map(|id| {
                crate::anthropic::TurnProgressEvent {
                    id: id.to_owned(),
                    title: "SubAgent launch awaiting prompt".to_owned(),
                    status: "dropped".to_owned(),
                    elapsed_ms,
                }
            }))
            .collect()
    }

    fn progress_event(
        &self,
        id: &str,
        title: &str,
        elapsed_ms: u64,
    ) -> crate::anthropic::TurnProgressEvent {
        let completed = self.provider_tool_terminal_ids.contains(id);
        crate::anthropic::TurnProgressEvent {
            id: id.to_owned(),
            title: title.to_owned(),
            status: if completed {
                "completed"
            } else {
                "in_progress"
            }
            .to_owned(),
            elapsed_ms,
        }
    }

    pub(in crate::anthropic::stream) fn publish_turn_progress(
        &self,
        session: &crate::anthropic::Session,
    ) {
        session.record_turn_progress(self.last_turn_progress.clone());
    }

    pub(in crate::anthropic::stream) fn next_sse_index(&self) -> usize {
        self.blocks.len()
    }

    pub(in crate::anthropic::stream) fn has_committed_output(&self) -> bool {
        if !self.pending_answer.is_empty() {
            return true;
        }
        if self
            .open_text_block
            .as_ref()
            .is_some_and(|(_, text)| !text.is_empty())
        {
            return true;
        }
        let mut blocks = self.blocks.clone();
        sanitize_committed_blocks(&mut blocks);
        !blocks.is_empty()
    }
}
