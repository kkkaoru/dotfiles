
use serde_json::Value;

use super::{
    is_launch_tool,
    records_scope::active_status,
    records_status::launch_record,
};

pub(super) use super::records_scope::summarize_scope;

mod transcript;
pub(super) use transcript::apply_transcript;

#[derive(Clone, Debug, Default, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub(super) struct LaunchRecord {
    pub(super) key: String,
    pub(super) recipient: String,
    #[serde(default)]
    pub(super) scope: String,
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default = "active_status")]
    pub(super) status: String,
}

pub(super) fn merge_launches<'a>(
    launches: &mut Vec<LaunchRecord>,
    observed: impl Iterator<Item = &'a LaunchRecord>,
) {
    for launch in observed {
        match launches.iter_mut().find(|current| {
            // Empty key/recipient are placeholders (inflight note before
            // tool_result). Matching ""=="" collapsed parallel SubAgents into
            // one record and broke occupancy / resume / spawn limits.
            (!current.key.is_empty() && current.key == launch.key)
                || (!current.recipient.is_empty()
                    && !launch.recipient.is_empty()
                    && current.recipient == launch.recipient)
                || (same_logical_launch(current, launch) && !terminal_status(&current.status))
        }) {
            Some(current) => merge_record(current, launch),
            None => launches.push(launch.clone()),
        }
    }
}

fn same_logical_launch(current: &LaunchRecord, observed: &LaunchRecord) -> bool {
    if current.scope.is_empty() || observed.scope.is_empty() {
        return false;
    }
    if normalize_scope(&current.scope) != normalize_scope(&observed.scope) {
        return false;
    }
    // Same agents-panel title with different claudex_model is intentional
    // multi-route fan-out (gpt + cursor + muse + advisor), not a duplicate.
    match (&current.model, &observed.model) {
        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
        (None, None) => true,
        _ => false,
    }
}

pub(super) fn launch_scope_key(input: &Value) -> String {
    normalize_scope(&super::records_scope::summarize_scope(input))
}

pub(super) fn launch_model(input: &Value) -> Option<&str> {
    input
        .get("claudex_model")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

pub(super) fn scope_is_occupied(
    launches: &[LaunchRecord],
    scope_key: &str,
    model: Option<&str>,
) -> bool {
    !scope_key.is_empty()
        && launches.iter().any(|launch| {
            if terminal_status(&launch.status) || normalize_scope(&launch.scope) != scope_key {
                return false;
            }
            match (model, launch.model.as_deref()) {
                (Some(want), Some(have)) => want.eq_ignore_ascii_case(have),
                (None, None) => true,
                // Model-less placeholders must not block explicit multi-model
                // fan-out; model-less queries still collide with any occupant.
                (None, Some(_)) => true,
                (Some(_), None) => false,
            }
        })
}

fn normalize_scope(scope: &str) -> String {
    scope
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

pub(super) fn terminal_status(status: &str) -> bool {
    matches!(
        status,
        "completed" | "failed" | "cancelled" | "canceled" | "timeout" | "stopped"
    )
}

pub(super) fn reusable_status(status: &str) -> bool {
    matches!(status, "active" | "message_queued" | "completed")
}

pub(super) fn already_has_resume(arguments: &Value) -> bool {
    arguments
        .get("resume")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
}

mod find;
mod live;
pub(super) use find::{find_reusable_launch, merge_record};
#[cfg(test)]
pub(super) use live::launch_records;
pub(in crate::anthropic) use live::live_agent_task_ids;
