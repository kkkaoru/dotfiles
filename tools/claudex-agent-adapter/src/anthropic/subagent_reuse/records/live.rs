use std::collections::HashSet;
#[cfg(test)]
use std::collections::HashMap;

use serde_json::Value;

use super::{LaunchRecord, apply_transcript, terminal_status};
#[cfg(test)]
use super::launch_record;
#[cfg(test)]
use super::transcript::remember_launch_context;

#[cfg(test)]
pub(in crate::anthropic::subagent_reuse) fn launch_records(messages: &[Value]) -> Vec<LaunchRecord> {
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

pub(super) fn push_live_candidates(
    ids: &mut Vec<String>,
    seen: &mut HashSet<String>,
    launch: &LaunchRecord,
) {
    for candidate in [&launch.recipient, &launch.key] {
        if !crate::anthropic::task_ids::is_claude_code_agent_task_id(candidate) {
            continue;
        }
        if seen.insert(candidate.to_ascii_lowercase()) {
            ids.push(candidate.clone());
        }
    }
}
