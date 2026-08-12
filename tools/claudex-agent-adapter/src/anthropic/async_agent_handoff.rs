use std::sync::Arc;

use axum::{body::Body, http::Response};

use super::{
    Bridge, MessagesRequest,
    content::{ToolResult, collect_turn_tool_results, trailing_user_messages},
    internal_notification,
};
pub(super) const ASYNC_LAUNCH_PREFIX: &str = "Async agent launched successfully.";
pub(super) const BACKGROUND_MARKER: &str = "The agent is working in the background.";

fn background_handoff_text(launch_count: usize) -> String {
    if launch_count == 1 {
        "Background agent launched; the main prompt is ready.".to_owned()
    } else {
        format!("{launch_count} background agents launched; the main prompt is ready.")
    }
}

/// Keep the parent provider turn open when Claude Code only acked a partial
/// fan-out. The scheduler owns exact multi-scope cardinality (one scope → one
/// worker), so handoff must use that bounded target instead of a second floor.
fn should_defer_background_handoff(request: &MessagesRequest, launch_count: usize) -> bool {
    use crate::parallel_scheduler::ParallelScheduler;

    should_defer_background_handoff_with(ParallelScheduler::shared(), request, launch_count)
}

fn should_defer_background_handoff_with(
    scheduler: &crate::parallel_scheduler::ParallelScheduler,
    request: &MessagesRequest,
    launch_count: usize,
) -> bool {
    // Claude Code may append a text-only steering user message after the async
    // ack. That path must still hand control back so the main prompt can react.
    if has_post_ack_steering(request) {
        return false;
    }
    launch_count < scheduler.decision_for_request(request).target_workers
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

impl Bridge {
    pub(super) async fn async_agent_launch_handoff(
        &self,
        request: &MessagesRequest,
    ) -> Option<Response<Body>> {
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
        if should_defer_background_handoff(request, tool_use_ids.len()) {
            tracing::info!(
                launch_count = tool_use_ids.len(),
                recorded_background,
                "deferring native background handoff until scheduler SubAgent target is met"
            );
            return None;
        }
        let results = collect_turn_tool_results(&request.messages);
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
        let ready = self
            .app_for_session(&session)
            .ensure_thread_ready(&session.thread_id)
            .await;
        if ready.is_err() {
            return false;
        }
        let events = Arc::new(
            self.app_for_session(&session)
                .subscribe_thread(&session.thread_id),
        );
        // Do not abort the shared Codex provider; reject and drain this turn.
        self.disconnect_stream_for_async_handoff(&session, events)
            .await;
        true
    }
}

#[path = "async_agent_handoff_parse.rs"]
mod parse;
pub(crate) use parse::{agent_tool_round_ids, exact_async_launch_acknowledgement};
#[cfg(test)]
use parse::{append_strict_result_text, async_launch_tool_results, strict_result_text};
use parse::{latest_agent_tool_round_ids, pending_tools_outside_async_launches};

#[cfg(test)]
#[path = "async_agent_handoff_extra_tests.rs"]
mod extra_tests;
