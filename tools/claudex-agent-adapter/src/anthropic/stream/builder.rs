use std::{collections::HashSet, ops::ControlFlow, time::Instant};

use anyhow::Result;
use serde_json::Value;

use super::{ToolCall, error_flow, turn_flow};
use crate::anthropic::{Bridge, Session, Usage};

use super::{protocol::StreamSender, thinking::ThinkingState};

mod batch;
mod close_finish;
mod construct;
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
    /// SubAgent AgentMessage prose held until end_turn. Live viewer sees compact
    /// prose on thinking after the ZWSP prime is closed.
    pub(super) pending_answer: String,
    /// ACP `AgentThoughtChunk` / Codex raw `textDelta` CoT. Live SSE streams the
    /// body after closing ZWSP; this buffer still replaces ▶ chrome at finish.
    pub(super) pending_reasoning: String,
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
    /// Launch-shaped cards that stayed incomplete (`_toolName` only / empty
    /// prompt). Reported visibly if they never bridge before turn end.
    incomplete_launch_call_ids: Vec<String>,
    /// Incomplete launch cards reported at turn end. Keep this separately from
    /// the pending IDs so `turn_progress` can truthfully say they were dropped.
    dropped_launch_call_ids: Vec<String>,
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
    pub(super) last_turn_progress: Vec<crate::anthropic::TurnProgressEvent>,
}

impl SegmentBuilder {
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
            Some("turn/completed") => {
                self.report_incomplete_launches(stream).await?;
                self.drain_remaining_queued_launches(
                    bridge,
                    session,
                    current_messages,
                    system,
                    stream,
                )
                .await?;
                return turn_flow(event);
            }
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
