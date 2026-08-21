use anyhow::Result;
use serde_json::{Value, json};

use super::SegmentBuilder;
use crate::anthropic::{Bridge, Session};

use super::super::protocol::{StreamSender, send_stream_frame};

impl SegmentBuilder {
    pub(super) async fn provider_launch_event(
        &mut self,
        _bridge: &Bridge,
        session: &Session,
        _current_messages: &[Value],
        _system: &Value,
        event: &Value,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let method = event.get("method").and_then(Value::as_str);
        let call_id = event
            .get("params")
            .and_then(|params| params.get("callId"))
            .and_then(Value::as_str);
        let params = event.get("params");
        let tool = params
            .and_then(|params| params.get("tool"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let title = params
            .and_then(|params| params.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let mcp_hint =
            call_id.is_some_and(|id| self.mcp_provider_call_ids.iter().any(|known| known == id));
        let mcp_shaped = mcp_hint
            || tool.eq_ignore_ascii_case("mcp")
            || title.to_ascii_lowercase().starts_with("mcp");
        if self.is_subagent
            && crate::anthropic::nested_subagent_launch::event_is_forbidden_nested_launch(event)
        {
            self.emit_incomplete_launch_notice(
                crate::anthropic::subagent_reuse::NESTED_SUBAGENT_LAUNCH_NOTICE,
                stream,
            )
            .await?;
            return Ok(());
        }
        if is_exact_native_launch_attempt(event) && session.take_launch_unavailable_notice() {
            self.emit_incomplete_launch_notice(
                "Claudex: Agent/Task is unavailable in this session, so this SubAgent was not started. Try another model or session that offers Agent/Task.",
                stream,
            )
            .await?;
        }
        self.note_mcp_provider_call(call_id, mcp_shaped);
        if self
            .should_suppress_unbridged_launch(session, event, mcp_hint, call_id, stream)
            .await?
        {
            return Ok(());
        }
        if method == Some("item/providerTool/call") {
            self.provider_tool_call(event, stream).await
        } else {
            self.provider_tool_update(event, stream).await
        }
    }

    fn note_mcp_provider_call(&mut self, call_id: Option<&str>, mcp_shaped: bool) {
        let Some(call_id) = call_id else {
            return;
        };
        if mcp_shaped && !self.mcp_provider_call_ids.iter().any(|id| id == call_id) {
            self.mcp_provider_call_ids.push(call_id.to_owned());
        }
    }

    async fn should_suppress_unbridged_launch(
        &mut self,
        session: &Session,
        event: &Value,
        mcp_hint: bool,
        call_id: Option<&str>,
        stream: Option<&StreamSender>,
    ) -> Result<bool> {
        if !mcp_hint {
            return Ok(false);
        }
        let Some(call_id) = call_id else {
            return Ok(true);
        };
        if session.launch_unavailable() {
            return Ok(true);
        }
        let newly_tracked = self.track_incomplete_launch(call_id);
        if newly_tracked && is_unconfirmed_task_launch(session, event) {
            self.emit_incomplete_launch_notice(
                "Claudex: This SubAgent launch is awaiting its prompt. Send Agent/Task again with a non-empty prompt in this turn.",
                stream,
            )
            .await?;
        }
        Ok(true)
    }

    fn track_incomplete_launch(&mut self, call_id: &str) -> bool {
        let already_known = self
            .incomplete_launch_call_ids
            .iter()
            .any(|id| id == call_id)
            || self
                .bridged_provider_launch_ids
                .iter()
                .any(|id| id == call_id);
        if !already_known {
            self.incomplete_launch_call_ids.push(call_id.to_owned());
        }
        !already_known
    }

    pub(super) async fn drain_remaining_queued_launches(
        &self,
        _bridge: &Bridge,
        _session: &Session,
        _current_messages: &[Value],
        _system: &Value,
        _stream: Option<&StreamSender>,
    ) -> Result<()> {
        Ok(())
    }

    pub(super) async fn report_incomplete_launches(
        &mut self,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        let pending = self
            .incomplete_launch_call_ids
            .iter()
            .filter(|id| {
                !self
                    .bridged_provider_launch_ids
                    .iter()
                    .any(|bridged| bridged == *id)
            })
            .cloned()
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return Ok(());
        }
        let message = format!(
            "Claudex: {} SubAgent launch card(s) never received a prompt and were not started (callIds: {}). Re-issue Agent/Task with a non-empty prompt in the same turn; `_toolName`-only cards do not start workers.",
            pending.len(),
            pending.join(", ")
        );
        tracing::warn!(
            incomplete = pending.len(),
            call_ids = %pending.join(","),
            "incomplete launch cards dropped without a prompt"
        );
        self.emit_incomplete_launch_notice(&message, stream).await?;
        self.dropped_launch_call_ids.extend(pending);
        self.incomplete_launch_call_ids.clear();
        Ok(())
    }

    async fn emit_incomplete_launch_notice(
        &mut self,
        notice: &str,
        stream: Option<&StreamSender>,
    ) -> Result<()> {
        self.close_open_blocks(stream).await?;
        self.note_provider_turn_activity();
        let index = self.start_text_block(notice, stream).await?;
        send_stream_frame(stream, "content_block_delta", || {
            json!({
                "type":"content_block_delta", "index":index,
                "delta":{"type":"text_delta","text":notice}
            })
        })
        .await?;
        self.close_text_block(stream).await
    }
}

fn is_unconfirmed_task_launch(session: &Session, event: &Value) -> bool {
    let Some(params) = event.get("params") else {
        return false;
    };
    ["tool", "title"].into_iter().any(|field| {
        let Some(candidate) = params.get(field).and_then(Value::as_str) else {
            return false;
        };
        candidate.eq_ignore_ascii_case("Task")
            || session
                .external_tool_names
                .get(candidate)
                .is_some_and(|name| name == "Task")
    }) || params
        .pointer("/arguments/_toolName")
        .and_then(Value::as_str)
        .is_some_and(|name| name.eq_ignore_ascii_case("Task"))
}

fn is_exact_native_launch_attempt(event: &Value) -> bool {
    let Some(params) = event.get("params") else {
        return false;
    };
    ["tool", "title"].into_iter().any(|field| {
        params
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|name| matches!(name, "Agent" | "Task"))
    }) || params
        .pointer("/arguments/_toolName")
        .and_then(Value::as_str)
        .is_some_and(|name| matches!(name, "Agent" | "Task"))
}
