use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::anthropic::{MessagesRequest, agent_tool_round_ids, exact_async_launch_acknowledgement};

mod parse;
use parse::{legacy_cc_agent_round_ids, record_message_content, record_task_notification};

#[derive(Clone, Debug)]
pub(crate) struct LiveThreadState {
    pub(crate) last_seen: std::time::Instant,
    pub(crate) last_reassessed: std::time::Instant,
    pub(crate) active_units: HashSet<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Workunit {
    pub(crate) unit_id: String,
    pub(crate) group_id: String,
    pub(crate) model: Option<String>,
}

#[derive(Default)]
pub(crate) struct SubagentSnapshot {
    pub(crate) active_unit_ids: HashSet<String>,
    pub(crate) active_models: HashMap<String, String>,
}

impl SubagentSnapshot {
    pub(crate) fn has_any_workers(&self) -> bool {
        !self.active_unit_ids.is_empty()
    }

    pub(crate) fn active_count(&self) -> usize {
        self.active_unit_ids.len()
    }

    pub(crate) fn active_model_families(&self) -> usize {
        self.active_models.values().collect::<HashSet<_>>().len()
    }
}

pub(crate) fn analyze_subagent_work(messages: &[Value]) -> SubagentSnapshot {
    let mut launched: Vec<Workunit> = Vec::new();
    let mut completed: HashSet<String> = HashSet::new();
    let mut latest_agent_tool_round: Option<Vec<String>> = None;

    for message in messages {
        record_task_notification(message, &mut completed);
        let Some(content) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        let exact_async_acknowledgement =
            latest_agent_tool_round.as_deref().is_some_and(|expected| {
                exact_async_launch_acknowledgement(message, expected).is_some()
            });

        record_message_content(
            content,
            &mut launched,
            &mut completed,
            exact_async_acknowledgement,
        );
        if message.get("role").and_then(Value::as_str) == Some("assistant")
            && let Some(ids) =
                agent_tool_round_ids(message).or_else(|| legacy_cc_agent_round_ids(message))
        {
            latest_agent_tool_round = Some(ids);
        }
    }

    let mut snapshot = SubagentSnapshot::default();
    for launch in launched {
        if completed.contains(&launch.unit_id) || completed.contains(&launch.group_id) {
            continue;
        }
        snapshot.active_unit_ids.insert(launch.unit_id.clone());
        if let Some(model) = launch.model {
            snapshot
                .active_models
                .insert(launch.unit_id, model_family(&model));
        }
    }

    snapshot
}

pub(crate) fn has_exact_async_launch_ack(messages: &[Value]) -> bool {
    let mut latest_agent_tool_round: Option<Vec<String>> = None;
    let mut acknowledged = false;
    for message in messages {
        acknowledged = latest_agent_tool_round.as_deref().is_some_and(|expected| {
            exact_async_launch_acknowledgement(message, expected).is_some()
        });
        if message.get("role").and_then(Value::as_str) == Some("assistant")
            && let Some(ids) =
                agent_tool_round_ids(message).or_else(|| legacy_cc_agent_round_ids(message))
        {
            latest_agent_tool_round = Some(ids);
        }
    }
    acknowledged
}

pub(crate) fn previous_completed(
    previous: &LiveThreadState,
    current_active: &HashSet<String>,
    previous_active_size: usize,
) -> usize {
    if previous_active_size == 0 {
        return 0;
    }
    previous
        .active_units
        .iter()
        .filter(|old| !current_active.contains(*old))
        .count()
}

fn model_family(model: &str) -> String {
    model
        .split(['/', '-', '_', '.'])
        .next()
        .filter(|part| !part.is_empty())
        .map_or_else(|| model.to_owned(), ToOwned::to_owned)
}

pub(crate) fn thread_key(request: &MessagesRequest) -> String {
    let tools = serde_json::to_string(&request.tools).unwrap_or_else(|_| String::from("[]"));
    let metadata = request
        .metadata
        .get("user_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut disabled = request
        .disabled_subagent_models
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    disabled.sort_unstable();

    format!(
        "{}|{}|{}|{}|{}|{}",
        metadata,
        request.model,
        request.system,
        tools,
        request.disabled_subagent_models.len(),
        disabled.join("|")
    )
}
