//! ACP provider tools → Claude Code `tool_use` only for client-executed tools.
//!
//! Provider-native tools (bash/read/edit/…) stay as WIP progress. Bridging them would
//! double-execute under Claude Code. Launch tools (`Agent`/`Task`) and Grok-native
//! `spawn_subagent` (mapped onto Agent when Claude Code supplied Agent) become
//! Anthropic `tool_use`.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::ToolCall;

mod detect;
mod launch;
#[allow(unused_imports)]
use detect::{has_agent_tool, requested_original_name};
use detect::{
    launch_arguments_ready, launch_tool_name_from_arguments, looks_like_launch_arguments,
    looks_like_launch_tool, map_launch_name, normalize_launch_arguments,
};
#[cfg(test)]
use launch::is_compact_tool_label;
use launch::{launch_name_candidates, looks_like_mcp_surface, trace_launch_shaped_event};

/// Pending-tool request id marker: ACP cannot accept Codex-style tool results on the
/// app-server channel; follow-up turns continue via transcript + `turn/start`.
pub(super) const ACP_BRIDGE_MARKER: &str = "acpBridge";

pub(super) fn is_client_executed_bridge_tool(original_name: &str) -> bool {
    matches!(original_name, "Agent" | "Task")
}

pub(in crate::anthropic) fn is_acp_bridge_request_id(id: &Value) -> bool {
    id.get(ACP_BRIDGE_MARKER).and_then(Value::as_bool) == Some(true)
}

pub(super) fn acp_bridge_request_id(call_id: &str) -> Value {
    json!({ ACP_BRIDGE_MARKER: true, "callId": call_id })
}

fn bridgeable_status(status: Option<&str>) -> bool {
    // Cursor often opens Task incomplete, then fills args on update/completed.
    // Allow completed so a late-ready launch still becomes Claude Code tool_use.
    match status.unwrap_or("pending") {
        "pending" | "in_progress" | "started" | "completed" => true,
        "failed" | "cancelled" => false,
        _ => true,
    }
}

/// True when this providerTool event is a launch-shaped Agent/Task/spawn_subagent
/// card that will not become Claude Code tool_use (incomplete args, or missing
/// Agent mapping). Suppress WIP text for those so Cursor `auto` cannot fake
/// `▶ Task` / `▶ MCP` / `✓ Task` as if Claudex workers started.
///
/// Failed/cancelled cards are not suppressed: the failure must stay visible so
/// `_toolName`-only launches are not silently dropped.
pub(super) fn is_unbridged_launch_progress(
    external_tool_names: &HashMap<String, String>,
    event: &Value,
) -> bool {
    let Some(params) = event.get("params") else {
        return false;
    };
    let status = params.get("status").and_then(Value::as_str);
    if matches!(status, Some("failed") | Some("cancelled")) {
        return false;
    }
    let tool = params.get("tool").and_then(Value::as_str).unwrap_or("");
    let title = params.get("title").and_then(Value::as_str).unwrap_or("");
    let raw_args = params.get("arguments").unwrap_or(&Value::Null);
    let launch_shaped = launch_name_candidates(tool, title)
        .into_iter()
        .any(|candidate| {
            looks_like_launch_tool(candidate)
                || map_launch_name(candidate, external_tool_names).is_some()
        })
        || looks_like_launch_arguments(raw_args);
    if !launch_shaped {
        return false;
    }
    bridge_provider_tool_call(external_tool_names, event).is_none()
}

/// Build a Claude Code Agent/Task tool_use from queued `claudex-launch` args.
pub(super) fn tool_call_from_launch_queue_arguments(
    external_tool_names: &HashMap<String, String>,
    call_id: &str,
    arguments: Value,
) -> Option<ToolCall> {
    let provider_label = arguments
        .get("_toolName")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or("Agent");
    let name = launch_tool_name_from_arguments(&arguments, external_tool_names)
        .or_else(|| map_launch_name(provider_label, external_tool_names))?;
    let normalized = normalize_launch_arguments(provider_label, &arguments);
    if !launch_arguments_ready(&normalized) {
        return None;
    }
    Some(ToolCall {
        call_id: call_id.to_owned(),
        name,
        arguments: normalized,
        request_id: acp_bridge_request_id(call_id),
    })
}

/// If this providerTool event is a request-supplied Agent/Task launch (or Grok
/// spawn_subagent when Agent is available), return a ToolCall for Claude Code.
pub(super) fn bridge_provider_tool_call(
    external_tool_names: &HashMap<String, String>,
    event: &Value,
) -> Option<ToolCall> {
    bridge_provider_tool_call_inner(external_tool_names, event, false, None)
}

/// Like [`bridge_provider_tool_call`], but also consults the MCP launch queue when
/// Cursor later emits a generic `provider tool` update for a known MCP call id.
pub(super) fn bridge_provider_tool_call_with_mcp_hint(
    external_tool_names: &HashMap<String, String>,
    event: &Value,
    launch_owner: Option<&str>,
) -> Option<ToolCall> {
    bridge_provider_tool_call_inner(external_tool_names, event, true, launch_owner)
}

fn bridge_provider_tool_call_inner(
    external_tool_names: &HashMap<String, String>,
    event: &Value,
    use_mcp_queue: bool,
    launch_owner: Option<&str>,
) -> Option<ToolCall> {
    trace_launch_shaped_event(event);
    let params = event.get("params")?;
    if !bridgeable_status(params.get("status").and_then(Value::as_str)) {
        return None;
    }
    let call_id = params.get("callId").and_then(Value::as_str)?;
    let tool = params.get("tool").and_then(Value::as_str).unwrap_or("");
    let title = params.get("title").and_then(Value::as_str).unwrap_or("");
    let raw_args = params.get("arguments").unwrap_or(&Value::Null);
    let mcp_shaped = use_mcp_queue || [tool, title].into_iter().any(looks_like_mcp_surface);
    let normalized_raw = normalize_launch_arguments("Agent", raw_args);
    // Only the explicit MCP-hint path has enough session context to consult a
    // launch queue.  The ordinary bridge path is also used by
    // `is_unbridged_launch_progress`; consulting the process-global queue there
    // could let an unrelated queued launch make an empty provider card look
    // bridgeable (and suppress the honest "awaiting prompt" progress).
    let queued = if use_mcp_queue && mcp_shaped && !launch_arguments_ready(&normalized_raw) {
        super::acp_launch_queue::peek_pending_launch_arguments_for(launch_owner)
    } else {
        None
    };
    let effective_args = queued.as_ref().unwrap_or(raw_args);
    let (provider_label, name) = [tool, title]
        .into_iter()
        .filter(|candidate| !candidate.is_empty())
        .find_map(|candidate| {
            map_launch_name(candidate, external_tool_names).map(|name| (candidate, name))
        })
        .or_else(|| {
            if !looks_like_launch_arguments(effective_args)
                && ![tool, title].into_iter().any(looks_like_launch_tool)
            {
                return None;
            }
            let name = launch_tool_name_from_arguments(effective_args, external_tool_names)?;
            let label = [tool, title]
                .into_iter()
                .find(|candidate| !candidate.is_empty())
                .unwrap_or("Agent");
            Some((label, name))
        })?;
    let arguments = normalize_launch_arguments(provider_label, effective_args);
    if !launch_arguments_ready(&arguments) {
        return None;
    }
    if queued.is_some() {
        let _ = super::acp_launch_queue::take_pending_launch_arguments_for(launch_owner);
        tracing::info!(
            launch_owner,
            "using queued claudex-launch MCP arguments for ACP bridge"
        );
    }
    Some(ToolCall {
        call_id: call_id.to_owned(),
        name,
        arguments,
        request_id: acp_bridge_request_id(call_id),
    })
}

#[cfg(test)]
include!("tests.rs");
