use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::super::records_status::{
    mark_failed_launch_result, queued_message_recipient, set_recipient_status, set_task_status,
    status_update,
};
use super::super::value_text;
use super::{LaunchRecord, is_launch_tool, launch_record, merge_launches, terminal_status};
use crate::anthropic::task_ids::is_task_stop_tool_name;

const SESSION_WIDE_STOP_MARKER: &str = "全て中断";
const PAUSED_STATUS: &str = "paused";
const STOPPED_STATUS: &str = "stopped";

pub(in crate::anthropic::subagent_reuse) fn apply_transcript(
    launches: &mut Vec<LaunchRecord>,
    messages: &[Value],
) {
    let mut contexts = HashMap::new();
    let mut pending_claude_stop = HashSet::new();
    let mut session_wide_stop = false;
    for message in messages {
        apply_session_wide_user_message(
            launches,
            &mut pending_claude_stop,
            &mut session_wide_stop,
            message,
        );
        remember_claude_task_stops(
            &mut pending_claude_stop,
            launches,
            message,
            session_wide_stop,
        );
        apply_message_content(launches, &mut contexts, message);
        if let Some((task_id, status)) = status_update(message) {
            // Pair TaskStop with the next status for that id only. An older
            // Claude TaskStop must not remap a later bare /tasks stop, and a
            // user session-wide stop must not be rewritten to paused.
            let pending = pending_claude_stop.remove(&task_id);
            let status = classify_stopped_status(&status, pending);
            set_task_status(launches, &task_id, status);
            continue;
        }
        if let Some(recipient) = queued_message_recipient(message) {
            set_recipient_status(launches, &recipient, "message_queued".to_owned());
        }
    }
}

fn apply_session_wide_user_message(
    launches: &mut [LaunchRecord],
    pending_claude_stop: &mut HashSet<String>,
    session_wide_stop: &mut bool,
    message: &Value,
) {
    if is_user_session_wide_stop(message) {
        *session_wide_stop = true;
        pending_claude_stop.clear();
        // 502-dead workers never receive TaskStop; still mark them terminal so
        // they do not occupy skip/rewrite after a user full-stop.
        stop_live_launches(launches);
        return;
    }
    if is_steering_user_text(message) {
        *session_wide_stop = false;
    }
}

fn classify_stopped_status(status: &str, pending_claude_stop: bool) -> String {
    if status == STOPPED_STATUS && pending_claude_stop {
        PAUSED_STATUS.to_owned()
    } else {
        status.to_owned()
    }
}

fn remember_claude_task_stops(
    pending_claude_stop: &mut HashSet<String>,
    launches: &mut [LaunchRecord],
    message: &Value,
    user_stopped: bool,
) {
    let Some(content) = message.get("content") else {
        return;
    };
    for block in content.as_array().into_iter().flatten() {
        remember_claude_task_stop_block(pending_claude_stop, launches, block, user_stopped);
    }
}

fn remember_claude_task_stop_block(
    pending_claude_stop: &mut HashSet<String>,
    launches: &mut [LaunchRecord],
    block: &Value,
    user_stopped: bool,
) {
    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
        return;
    }
    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
    if !is_task_stop_tool_name(name) {
        return;
    }
    let Some(task_id) = block
        .get("input")
        .and_then(|input| input.get("task_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    // Official TaskStop of one agent for a queued swap stays paused so
    // SendMessage may resume. A burst after the user asked 全て中断 is a
    // user stop and must not auto-resume.
    if user_stopped {
        set_task_status(launches, task_id, STOPPED_STATUS.to_owned());
        return;
    }
    if task_is_stopped(launches, task_id) {
        return;
    }
    pending_claude_stop.insert(task_id.to_owned());
    set_task_status(launches, task_id, PAUSED_STATUS.to_owned());
}

fn task_is_stopped(launches: &[LaunchRecord], task_id: &str) -> bool {
    launches.iter().any(|launch| {
        (launch.key == task_id || launch.recipient == task_id) && launch.status == STOPPED_STATUS
    })
}

fn stop_live_launches(launches: &mut [LaunchRecord]) {
    for launch in launches {
        if terminal_status(&launch.status) {
            continue;
        }
        launch.status = STOPPED_STATUS.to_owned();
    }
}

fn is_user_session_wide_stop(message: &Value) -> bool {
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return false;
    }
    if message_has_tool_result(message) || status_update(message).is_some() {
        return false;
    }
    value_text(message.get("content")).contains(SESSION_WIDE_STOP_MARKER)
}

fn is_steering_user_text(message: &Value) -> bool {
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return false;
    }
    if message_has_tool_result(message) || status_update(message).is_some() {
        return false;
    }
    !value_text(message.get("content")).trim().is_empty()
}

fn message_has_tool_result(message: &Value) -> bool {
    message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
}

pub(super) fn apply_message_content(
    launches: &mut Vec<LaunchRecord>,
    contexts: &mut HashMap<String, (String, Option<String>, Option<String>)>,
    message: &Value,
) {
    let Some(content) = message.get("content") else {
        return;
    };
    for block in content.as_array().into_iter().flatten() {
        remember_launch_context(contexts, block);
        match launch_record(block, contexts) {
            Some(record) => merge_launches(launches, std::iter::once(&record)),
            None => mark_failed_launch_result(launches, contexts, block),
        }
    }
}

pub(in crate::anthropic::subagent_reuse) fn remember_launch_context(
    contexts: &mut HashMap<String, (String, Option<String>, Option<String>)>,
    block: &Value,
) {
    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
        return;
    }
    let Some(name) = block.get("name").and_then(Value::as_str) else {
        return;
    };
    if !is_launch_tool(name) && name != "SendMessage" {
        return;
    }
    let Some(id) = block.get("id").and_then(Value::as_str) else {
        return;
    };
    let input = block.get("input").unwrap_or(&Value::Null);
    let resume = ["to", "resume", "resume_from"].iter().find_map(|key| {
        input
            .get(*key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    });
    contexts.insert(
        id.to_owned(),
        (
            super::super::records_scope::summarize_scope(input),
            input
                .get("claudex_model")
                .and_then(Value::as_str)
                .map(str::to_owned),
            resume,
        ),
    );
}
