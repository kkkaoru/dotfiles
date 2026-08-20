use serde_json::{Value, json};

use super::{is_launch_tool, records_scope::active_status, records_status::launch_record};

const FOLLOW_UP_RECIPIENT_KEYS: [&str; 3] = ["to", "resume", "resume_from"];
const FOLLOW_UP_MESSAGE_KEYS: [&str; 2] = ["message", "prompt"];

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
    if same_scope_title(&current.scope, &observed.scope) {
        return models_match_exact(current.model.as_deref(), observed.model.as_deref());
    }
    super::records_scope::share_source_path(&current.scope, &observed.scope)
        && models_match_family(current.model.as_deref(), observed.model.as_deref())
}

pub(super) fn launch_scope_key(input: &Value) -> String {
    normalize_scope(&super::records_scope::summarize_scope(input))
}

pub(in crate::anthropic) fn launch_model(input: &Value) -> Option<&str> {
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
            !terminal_status(&launch.status)
                && occupancy_matches(&launch.scope, launch.model.as_deref(), scope_key, model)
        })
}

pub(in crate::anthropic::subagent_reuse) fn occupancy_matches(
    existing_scope: &str,
    existing_model: Option<&str>,
    proposed_scope: &str,
    proposed_model: Option<&str>,
) -> bool {
    if existing_scope.is_empty() || proposed_scope.is_empty() {
        return false;
    }
    if same_scope_title(existing_scope, proposed_scope) {
        return occupancy_models_match(existing_model, proposed_model);
    }
    super::records_scope::share_source_path(existing_scope, proposed_scope)
        && occupancy_family_matches(existing_model, proposed_model)
}

fn same_scope_title(left: &str, right: &str) -> bool {
    let left_title = super::records_scope::title_key(left);
    let right_title = super::records_scope::title_key(right);
    !left_title.is_empty() && left_title == right_title
}

fn occupancy_models_match(existing: Option<&str>, proposed: Option<&str>) -> bool {
    match (proposed, existing) {
        (Some(want), Some(have)) => want.eq_ignore_ascii_case(have),
        (None, Some(_)) | (None, None) => true,
        (Some(_), None) => false,
    }
}

fn occupancy_family_matches(existing: Option<&str>, proposed: Option<&str>) -> bool {
    match (proposed, existing) {
        (Some(want), Some(have)) => model_family(want).eq_ignore_ascii_case(&model_family(have)),
        (None, Some(_)) | (None, None) => true,
        (Some(_), None) => false,
    }
}

fn models_match_exact(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
        (None, None) => true,
        _ => false,
    }
}

fn models_match_family(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => model_family(left).eq_ignore_ascii_case(&model_family(right)),
        (None, None) => true,
        _ => false,
    }
}

fn model_family(model: &str) -> String {
    model
        .split(['/', '-', '_', '.'])
        .next()
        .filter(|part| !part.is_empty())
        .map_or_else(|| model.to_ascii_lowercase(), str::to_ascii_lowercase)
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
    matches!(status, "active" | "message_queued" | "completed" | "paused")
}

pub(super) enum ShadowCandidate<'a> {
    Selected(&'a LaunchRecord),
    ScopeUnknown,
    NoReusableRecord,
    ScopeMismatch,
}

pub(super) fn shadow_candidate<'a>(
    launches: &'a [LaunchRecord],
    arguments: &Value,
) -> ShadowCandidate<'a> {
    if let Some(selected) = find_reusable_launch(launches, arguments) {
        return ShadowCandidate::Selected(selected);
    }
    if summarize_scope(arguments).is_empty() {
        return ShadowCandidate::ScopeUnknown;
    }
    if launches
        .iter()
        .any(|launch| reusable_status(&launch.status) && !launch.recipient.is_empty())
    {
        ShadowCandidate::ScopeMismatch
    } else {
        ShadowCandidate::NoReusableRecord
    }
}

#[cfg(test)]
pub(in crate::anthropic) fn already_has_resume(arguments: &Value) -> bool {
    explicit_follow_up_recipient(arguments).is_some()
}

pub(super) fn explicit_follow_up_recipient(arguments: &Value) -> Option<String> {
    first_nonempty_string(arguments, &FOLLOW_UP_RECIPIENT_KEYS)
}

pub(super) fn follow_up_message(arguments: &Value) -> Option<String> {
    first_nonempty_string(arguments, &FOLLOW_UP_MESSAGE_KEYS)
}

pub(in crate::anthropic) fn is_send_message_follow_up(arguments: &Value) -> bool {
    arguments
        .get("to")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
        && arguments
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
}

pub(in crate::anthropic) fn has_listed_send_message<I, S>(names: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    names.into_iter().any(|name| name.as_ref() == "SendMessage")
}

pub(super) fn send_message_follow_up_arguments(
    recipient: &str,
    message: &str,
    summary: Option<&str>,
) -> Value {
    match summary {
        Some(summary) => json!({"to": recipient, "message": message, "summary": summary}),
        None => json!({"to": recipient, "message": message}),
    }
}

fn first_nonempty_string(arguments: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        arguments
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

mod find;
mod live;
pub(super) use find::{find_reusable_launch, merge_record};
#[cfg(test)]
pub(super) use live::launch_records;
pub(in crate::anthropic) use live::live_agent_task_ids;
pub(super) use live::unique_live_agent_count;
