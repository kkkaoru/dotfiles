use axum::{body::Body, http::Response};

use super::{Bridge, MessagesRequest, content::trailing_user_messages, internal_notification};
pub(super) const ASYNC_LAUNCH_PREFIX: &str = "Async agent launched successfully.";
pub(super) const BACKGROUND_MARKER: &str = "The agent is working in the background.";

fn background_handoff_text(launch_count: usize) -> String {
    if launch_count == 1 {
        "Background agent launched; the main prompt is ready.".to_owned()
    } else {
        format!("{launch_count} background agents launched; the main prompt is ready.")
    }
}

/// Keep the parent provider turn open only for a post-ack steering user
/// message. A completed launch-ack round is short status, not another
/// generation that launches more workers after launch chrome.
fn should_defer_background_handoff(request: &MessagesRequest, _launch_count: usize) -> bool {
    should_defer_background_handoff_with(request)
}

fn should_defer_background_handoff_with(request: &MessagesRequest) -> bool {
    // Claude Code may append a text-only steering user message after the async
    // ack. Defer the chrome handoff so steering reaches the parent provider
    // turn instead of skipping it.
    has_post_ack_steering(request)
}

fn has_post_ack_steering(request: &MessagesRequest) -> bool {
    trailing_user_messages(&request.messages)
        .iter()
        .any(is_text_only_user_message)
}

fn is_text_only_user_message(message: &serde_json::Value) -> bool {
    match message.get("content") {
        Some(serde_json::Value::String(text)) => !text.trim().is_empty(),
        Some(serde_json::Value::Array(blocks)) => {
            let has_text = blocks.iter().any(|block| {
                block.get("type").and_then(serde_json::Value::as_str) == Some("text")
                    && block
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty())
            });
            let has_tool_result = blocks.iter().any(|block| {
                block.get("type").and_then(serde_json::Value::as_str) == Some("tool_result")
            });
            has_text && !has_tool_result
        }
        _ => false,
    }
}

pub(crate) fn acknowledged_background_launch_count(request: &MessagesRequest) -> Option<usize> {
    let round_ids = latest_agent_tool_round_ids(request)?;
    trailing_user_messages(&request.messages)
        .iter()
        .find_map(|message| exact_async_launch_acknowledgement(message, &round_ids))
        .map(|ids| ids.len())
}

fn completed_launch_ack_status(request: &MessagesRequest) -> Option<Response<Body>> {
    if super::agent_effort::is_subagent_request(request) {
        return None;
    }
    let count = acknowledged_background_launch_count(request)?;
    if should_defer_background_handoff(request, count) {
        return None;
    }
    tracing::info!(
        launch_count = count,
        "skipping provider generation after completed native background launch ack"
    );
    Some(internal_notification::acknowledge_with_text(
        request,
        &background_handoff_text(count),
    ))
}

impl Bridge {
    pub(super) fn launch_ack_without_provider_turn(
        &self,
        request: &MessagesRequest,
    ) -> Option<Response<Body>> {
        let _ = self.async_agent_launch_handoff(request);
        completed_launch_ack_status(request)
    }

    pub(super) fn async_agent_launch_handoff(
        &self,
        request: &MessagesRequest,
    ) -> Option<Response<Body>> {
        // Nested Agent launches inside an implementing SubAgent must keep that
        // parent turn open. The background-ack chrome is not the worker result.
        if super::agent_effort::is_subagent_request(request) {
            return None;
        }
        let round_tool_use_ids = latest_agent_tool_round_ids(request)?;
        // Ack may sit on an earlier trailing user message when Claude Code
        // appends a text-only steering message after the tool_results.
        let tool_use_ids = trailing_user_messages(&request.messages)
            .iter()
            .find_map(|message| exact_async_launch_acknowledgement(message, &round_tool_use_ids))?;
        // Claude Code's async launch acknowledgement is authoritative. Recorded
        // agent_efforts intents are optional telemetry only: SubAgent startup may
        // already have matched routing state, and public tool args may have
        // defaulted run_in_background after the intent was recorded.
        let recorded_background = self
            .agent_efforts
            .background_launches(&tool_use_ids)
            .is_some();
        let defer = should_defer_background_handoff(request, tool_use_ids.len());
        tracing::info!(
            launch_count = tool_use_ids.len(),
            recorded_background,
            defer,
            "native Claude Code background Agent launch ack is not parent completion"
        );
        // ASYNC_LAUNCH_PREFIX + BACKGROUND_MARKER is launch chrome, not a
        // completed parent turn. Do not cancel or disconnect: the parent must
        // receive a real provider turn so it can SendMessage.
        None
    }
}

#[path = "async_agent_handoff_parse.rs"]
mod parse;
use parse::latest_agent_tool_round_ids;
pub(crate) use parse::{agent_tool_round_ids, exact_async_launch_acknowledgement};
#[cfg(test)]
use parse::{append_strict_result_text, async_launch_tool_results, strict_result_text};

#[cfg(test)]
#[path = "async_agent_handoff_extra_tests.rs"]
mod extra_tests;
