use std::collections::HashSet;
#[cfg(test)]
use std::collections::HashMap;

use serde_json::Value;

use super::{
    is_launch_tool,
    records_scope::active_status,
    records_status::launch_record,
};

pub(super) use super::records_scope::summarize_scope;

mod transcript;
pub(super) use transcript::apply_transcript;
#[cfg(test)]
use transcript::remember_launch_context;

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

pub(super) fn find_reusable_launch<'a>(
    launches: &'a [LaunchRecord],
    arguments: &Value,
) -> Option<&'a LaunchRecord> {
    let scope = summarize_scope(arguments);
    if scope.is_empty() {
        return None;
    }
    let model = arguments
        .get("claudex_model")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let proposed = LaunchRecord {
        key: String::new(),
        recipient: String::new(),
        scope,
        model,
        status: active_status(),
    };
    let mut exact = launches
        .iter()
        .enumerate()
        .filter(|(_, current)| {
            reusable_status(&current.status)
                && !current.recipient.is_empty()
                && same_logical_launch(current, &proposed)
        })
        .collect::<Vec<_>>();
    if exact.is_empty() {
        return None;
    }
    // Prefer higher status priority, then the newest record (prompt-cache /
    // latest transcript) when priorities tie.
    exact.sort_by(|(left_idx, left), (right_idx, right)| {
        reuse_priority(&right.status)
            .cmp(&reuse_priority(&left.status))
            .then_with(|| right_idx.cmp(left_idx))
    });
    exact.into_iter().next().map(|(_, launch)| launch)
}

fn reuse_priority(status: &str) -> u8 {
    match status {
        "active" => 2,
        "message_queued" => 1,
        "completed" => 0,
        _ => 0,
    }
}

fn merge_record(current: &mut LaunchRecord, observed: &LaunchRecord) {
    if current.recipient.is_empty() && !observed.recipient.is_empty() {
        current.recipient.clone_from(&observed.recipient);
    }
    // Explicit resume / follow-up launches refresh scope so the next same-scope
    // rewrite still hits this recipient instead of spawning a peer.
    if !observed.scope.is_empty() {
        current.scope.clone_from(&observed.scope);
    }
    if current.model.is_none() {
        current.model.clone_from(&observed.model);
    }
    if !observed.status.is_empty() {
        current.status.clone_from(&observed.status);
    }
}
#[cfg(test)]
pub(super) fn launch_records(messages: &[Value]) -> Vec<LaunchRecord> {
    let mut records = Vec::new();
    let mut contexts = HashMap::new();
    for message in messages {
        let Some(content) = message.get("content") else {
            continue;
        };
        let blocks = content.as_array().into_iter().flatten();
        for block in blocks {
            remember_launch_context(&mut contexts, block);
            records.extend(launch_record(block, &contexts));
        }
    }
    records
}

pub(in crate::anthropic) fn live_agent_task_ids(messages: &[Value]) -> Vec<String> {
    let mut launches = Vec::new();
    apply_transcript(&mut launches, messages);
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    for launch in launches {
        if terminal_status(&launch.status) {
            continue;
        }
        push_live_candidates(&mut ids, &mut seen, &launch);
    }
    ids
}

fn push_live_candidates(ids: &mut Vec<String>, seen: &mut HashSet<String>, launch: &LaunchRecord) {
    for candidate in [&launch.recipient, &launch.key] {
        if !crate::anthropic::task_ids::is_claude_code_agent_task_id(candidate) {
            continue;
        }
        if seen.insert(candidate.to_ascii_lowercase()) {
            ids.push(candidate.clone());
        }
    }
}
