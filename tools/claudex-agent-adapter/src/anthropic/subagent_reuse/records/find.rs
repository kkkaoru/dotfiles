use serde_json::Value;

use super::{LaunchRecord, active_status, reusable_status, same_logical_launch, summarize_scope};

pub(in crate::anthropic) fn find_reusable_launch<'a>(
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
        "completed" | "stopped" | "paused" => 0,
        _ => 0,
    }
}

pub(in crate::anthropic) fn merge_record(current: &mut LaunchRecord, observed: &LaunchRecord) {
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
