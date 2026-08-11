use std::{collections::HashSet, ops::ControlFlow, time::Instant};

use anyhow::Result;
use serde_json::Value;

use super::{ToolCall, error_flow, turn_flow};
use crate::anthropic::{Bridge, Session, Usage};

use super::{protocol::StreamSender, sanitize::sanitize_committed_blocks, thinking::ThinkingState};

mod batch;
mod close_finish;
mod external_tool;
mod external_tool_reject;
mod progress;
mod progress_keepalive;
mod provider_launch;
mod visibility;
#[path = "web_provenance.rs"]
mod web_provenance;

pub(super) use super::tool_call_parser::parse_tool_call;

pub(in crate::anthropic) struct SegmentBuilder {
    pub(super) blocks: Vec<Value>,
    pub(super) thinking: ThinkingState,
    pub(super) open_text_block: Option<(usize, String)>,
    /// SubAgent AgentMessage prose held until end_turn. Live viewer sees it as
    /// thinking; streaming `text_delta` now would hide the panel until finish.
    pub(super) pending_answer: String,
    pub(super) is_subagent: bool,
    /// Command Code SubAgent: keep native thinking open (no Thought-for flicker).
    /// Do not dump canned ▶/still-working text chrome.
    paint_command_code_progress: bool,
    turn_started_at: Instant,
    /// Last decoded provider event that counts as real progress (not synthetic
    /// keepalive). SubAgent silence judgment keys off this instant.
    last_visible_provider_at: Instant,
    external_tool_calls: usize,
    /// Provider call IDs already shown as progress text. ACP can report the same
    /// call first as ToolCall and again as a populated ToolCallUpdate.
    pub(super) provider_tool_calls: Vec<(String, String)>,
    /// Provider call IDs whose terminal status marker was already painted.
    /// ACP reconnect/replay can repeat the final ToolCallUpdate.
    pub(super) provider_tool_terminal_ids: HashSet<String>,
    /// True after the provider painted mid-turn output (Status / prose / ▶).
    /// Early Claude Code SSE drops after `message_start` must keep ACP alive;
    /// a later Stop once the worker is visibly working cancels the leaf.
    saw_provider_turn_activity: bool,
    /// Launch-shaped provider tools already bridged to Claude Code tool_use.
    /// Cursor may emit call → update → completed for the same callId.
    bridged_provider_launch_ids: Vec<String>,
    /// Cursor MCP launch callIds seen with empty args. Later generic
    /// `provider tool` updates for these ids still consult the MCP launch queue.
    mcp_provider_call_ids: Vec<String>,
    /// One-line hint already painted for a bulky JSON/tool dump this turn.
    bulk_dump_hinted: bool,
    requires_verified_web_evidence: bool,
    /// Completed provider-native web calls whose provenance has already been
    /// counted. A provider may repeat its final ToolCallUpdate while reconnecting.
    verified_web_evidence_call_ids: Vec<String>,
    /// Codex itemIds that already streamed `summaryTextDelta`. Raw `textDelta`
    /// for those items stays hidden so summaries are not duplicated.
    summarized_reasoning_ids: Vec<String>,
    injected_output_tokens: u64,
    usage: Usage,
}

impl SegmentBuilder {
    pub(super) fn new(input_tokens: u64) -> Self {
        Self {
            blocks: Vec::new(),
            thinking: ThinkingState::default(),
            open_text_block: None,
            pending_answer: String::new(),
            is_subagent: false,
            paint_command_code_progress: false,
            turn_started_at: Instant::now(),
            last_visible_provider_at: Instant::now(),
            external_tool_calls: 0,
            provider_tool_calls: Vec::new(),
            provider_tool_terminal_ids: HashSet::new(),
            saw_provider_turn_activity: false,
            bridged_provider_launch_ids: Vec::new(),
            mcp_provider_call_ids: Vec::new(),
            bulk_dump_hinted: false,
            requires_verified_web_evidence: false,
            verified_web_evidence_call_ids: Vec::new(),
            summarized_reasoning_ids: Vec::new(),
            injected_output_tokens: 0,
            usage: Usage {
                input_tokens,
                ..Usage::default()
            },
        }
    }

    pub(super) fn with_subagent(mut self, is_subagent: bool) -> Self {
        self.is_subagent = is_subagent;
        self
    }

    pub(super) fn with_command_code_progress(mut self, enabled: bool) -> Self {
        self.paint_command_code_progress = enabled;
        self
    }

    #[cfg(test)]
    pub(in crate::anthropic::stream) fn age_turn_for_test(&mut self, age: std::time::Duration) {
        self.turn_started_at = Instant::now() - age;
    }

    pub(super) fn for_turn(input_tokens: u64, is_subagent: bool, model: &str) -> Self {
        Self::new(input_tokens)
            .with_subagent(is_subagent)
            .with_command_code_progress(
                is_subagent && crate::command_code_acp::is_command_code_model(model),
            )
    }

    pub(super) fn is_command_code_subagent(&self) -> bool {
        self.is_subagent && self.paint_command_code_progress
    }

    pub(super) fn with_primed_thinking(mut self) -> Self {
        self.thinking.prime_silent_heartbeat(&mut self.blocks);
        self
    }

    pub(super) fn has_external_tool_calls(&self) -> bool {
        self.external_tool_calls > 0
    }

    /// True once the SubAgent has painted provider ▶ tools, bridged Claude
    /// tool_use, or other provider turn output (Status / answer chrome). Early
    /// Claude Code SSE drops (message_start only) must keep ACP alive; a later
    /// drop after live work is treated as user stop/interrupt.
    pub(super) fn has_live_provider_work(&self) -> bool {
        self.external_tool_calls > 0
            || !self.provider_tool_calls.is_empty()
            || self.saw_provider_turn_activity
    }

    pub(in crate::anthropic::stream::builder) fn note_provider_turn_activity(&mut self) {
        self.saw_provider_turn_activity = true;
        self.note_visible_provider_activity();
    }

    pub(super) fn note_visible_provider_activity(&mut self) {
        self.last_visible_provider_at = Instant::now();
    }

    #[cfg(test)]
    pub(super) fn backdate_last_visible_provider_activity(&mut self, elapsed: std::time::Duration) {
        self.last_visible_provider_at = Instant::now()
            .checked_sub(elapsed)
            .expect("test backdate must fit within Instant range");
    }

    /// Activity-based SubAgent bound: synthetic keepalives do not refresh this.
    pub(super) fn subagent_provider_silence_exceeded(&self, judgment: std::time::Duration) -> bool {
        self.is_subagent
            && self
                .last_visible_provider_at
                .elapsed()
                .checked_sub(judgment)
                .is_some()
    }

    pub(super) fn has_committed_output(&self) -> bool {
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

    pub(super) async fn handle_event(
        &mut self,
        bridge: &Bridge,
        session: &Session,
        current_messages: &[Value],
        system: &Value,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<ControlFlow<()>> {
        self.record_web_evidence_requirement(current_messages, system);
        if self.model_output_event(event, stream).await? {
            return Ok(ControlFlow::Continue(()));
        }
        match event.get("method").and_then(Value::as_str) {
            Some("item/tool/call") => {
                let call = parse_tool_call(event)?;
                self.tool_call(bridge, session, current_messages, system, call, stream)
                    .await?;
            }
            Some("item/providerTool/call") | Some("item/providerTool/update") => {
                self.provider_launch_event(
                    bridge,
                    session,
                    current_messages,
                    system,
                    event,
                    stream,
                )
                .await?;
            }
            Some("item/started") => {
                self.native_web_search_event(event, stream).await?;
            }
            Some("item/completed") => {
                self.native_web_search_event(event, stream).await?;
            }
            Some("thread/tokenUsage/updated") => self.update_usage(event),
            Some("error") => return error_flow(event),
            Some("turn/completed") => return turn_flow(event),
            _ => {}
        }
        Ok(ControlFlow::Continue(()))
    }

    pub(super) async fn model_output_event(
        &mut self,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<bool> {
        match event.get("method").and_then(Value::as_str) {
            Some("item/agentMessage/delta") => self.text_delta(event, stream).await?,
            Some("item/reasoning/summaryTextDelta") => {
                self.note_summarized_reasoning(event);
                self.reasoning_delta(event, stream).await?;
            }
            Some("item/reasoning/textDelta") => {
                self.raw_reasoning_delta(event, stream).await?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

#[cfg(test)]
// Coverage gates measure production behavior; this inline test is excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
