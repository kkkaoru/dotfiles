use std::{collections::HashSet, sync::Arc};

use axum::{body::Body, http::Response};
use serde_json::Value;

use super::{
    Bridge, MessagesRequest,
    content::{ToolResult, collect_tool_results},
    internal_notification,
};
const ASYNC_LAUNCH_PREFIX: &str = "Async agent launched successfully.";
const BACKGROUND_MARKER: &str = "The agent is working in the background.";

fn background_handoff_text(launch_count: usize) -> String {
    if launch_count == 1 {
        "Background agent launched; the main prompt is ready.".to_owned()
    } else {
        format!("{launch_count} background agents launched; the main prompt is ready.")
    }
}

impl Bridge {
    pub(super) async fn async_agent_launch_handoff(
        &self,
        request: &MessagesRequest,
    ) -> Option<Response<Body>> {
        let round_tool_use_ids = latest_agent_tool_round_ids(request)?;
        let message = request.messages.last()?;
        let tool_use_ids = exact_async_launch_acknowledgement(message, &round_tool_use_ids)?;
        // Claude Code's async launch acknowledgement is authoritative. Recorded
        // agent_efforts intents are optional telemetry only: SubAgent startup may
        // already have matched routing state, and public tool args may have
        // defaulted run_in_background after the intent was recorded.
        let recorded_background = self
            .agent_efforts
            .background_launches(&tool_use_ids)
            .is_some();
        let results = collect_tool_results(std::slice::from_ref(request.messages.last()?));
        if !self
            .cancel_handed_off_provider_session(&results, &tool_use_ids)
            .await
        {
            return None;
        }
        tracing::info!(
            launch_count = tool_use_ids.len(),
            recorded_background,
            "returned control after native Claude Code background Agent launch"
        );
        // Claude Code renders the launch/result in its native task panel from
        // the tool result. A visible, concise acknowledgement is still
        // required: an empty end_turn makes Claude Code inject a synthetic
        // "previous response had no visible output" user message and start a
        // duplicate provider turn, which queues the next user input.
        let text = background_handoff_text(tool_use_ids.len());
        Some(internal_notification::acknowledge_with_text(request, &text))
    }
    async fn cancel_handed_off_provider_session(
        &self,
        results: &[ToolResult],
        async_launch_ids: &[String],
    ) -> bool {
        let Some(session) = self.find_result_session(results).await else {
            return true;
        };
        let pending_ids = session
            .pending_tools
            .lock()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        if !pending_ids.is_empty()
            && pending_tools_outside_async_launches(&pending_ids, async_launch_ids)
        {
            // Only disconnect when every still-pending tool belongs to this
            // background launch acknowledgement. Leftover non-agent tools must
            // keep the provider turn open.
            return false;
        }
        let ready = self.app.ensure_thread_ready(&session.thread_id).await;
        if ready.is_err() {
            return false;
        }
        let events = Arc::new(self.app.subscribe_thread(&session.thread_id));
        // Do not abort the shared Codex provider; reject and drain this turn.
        self.disconnect_stream_for_async_handoff(&session, events)
            .await;
        true
    }
}

fn pending_tools_outside_async_launches(pending_ids: &[String], async_launch_ids: &[String]) -> bool {
    let async_set = async_launch_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    pending_ids
        .iter()
        .any(|pending_id| !async_set.contains(pending_id.as_str()))
}

/// Collect successful async background-launch tool_result IDs from a user
/// message. Non-result blocks and ordinary tool results are ignored so a mixed
/// Claude Code continuation can still hand control back once every Agent/Task
/// in the latest round is acknowledged as backgrounded.
fn async_launch_tool_results(message: &Value) -> Option<Vec<String>> {
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let blocks = message.get("content")?.as_array()?;
    if blocks.is_empty() {
        return None;
    }
    let mut ids = Vec::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool_result")
            || block.get("is_error").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let Some(text) = block.get("content").and_then(strict_result_text) else {
            continue;
        };
        if !text.trim_start().starts_with(ASYNC_LAUNCH_PREFIX) || !text.contains(BACKGROUND_MARKER)
        {
            continue;
        }
        let Some(id) = block
            .get("tool_use_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
        else {
            continue;
        };
        ids.push(id);
    }
    (!ids.is_empty()).then_some(ids)
}

/// Return the successful async launch IDs only when they are a duplicate-free,
/// exact match for the expected tool round. Both handoff and the
/// scheduler use this predicate so a partial or replayed acknowledgement cannot
/// make the two lifecycle views diverge.
pub(crate) fn exact_async_launch_acknowledgement(
    message: &Value,
    expected_tool_use_ids: &[String],
) -> Option<Vec<String>> {
    let result_ids = async_launch_tool_results(message)?;
    if result_ids.len() != expected_tool_use_ids.len() || result_ids.is_empty() {
        return None;
    }
    let result_set = result_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let expected_set = expected_tool_use_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if result_set.len() != result_ids.len()
        || expected_set.len() != expected_tool_use_ids.len()
        || result_set != expected_set
    {
        return None;
    }
    Some(result_ids)
}

fn strict_result_text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) if !items.is_empty() => {
            let mut text = String::new();
            for item in items {
                append_strict_result_text(&mut text, item)?;
            }
            Some(text)
        }
        _ => None,
    }
}

fn append_strict_result_text(text: &mut String, item: &Value) -> Option<()> {
    (item.get("type").and_then(Value::as_str) == Some("text")).then_some(())?;
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(item.get("text").and_then(Value::as_str)?);
    Some(())
}

fn latest_agent_tool_round_ids(request: &MessagesRequest) -> Option<Vec<String>> {
    request
        .messages
        .iter()
        .rev()
        .skip(1)
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .find_map(agent_tool_round_ids)
}

pub(crate) fn agent_tool_round_ids(message: &Value) -> Option<Vec<String>> {
    let ids = message
        .get("content")?
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter(|block| {
            block
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(super::agent_effort::is_agent_tool)
        })
        .map(|block| {
            block
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()?;
    (!ids.is_empty()).then_some(ids)
}

#[cfg(test)]
#[path = "async_agent_handoff_extra_tests.rs"]
mod extra_tests;
