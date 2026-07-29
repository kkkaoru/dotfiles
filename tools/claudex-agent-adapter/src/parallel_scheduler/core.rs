use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::anthropic::MessagesRequest;

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

    for message in messages {
        let Some(content) = message.get("content").and_then(Value::as_array) else {
            continue;
        };

        for block in content {
            let Some(block_type) = block.get("type").and_then(Value::as_str) else {
                continue;
            };
            match block_type {
                "tool_use" => parse_subagent_tool_use(block, &mut launched),
                "tool_result" => record_completed_tool(block, &mut completed),
                _ => {}
            }
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

fn record_completed_tool(block: &Value, completed: &mut HashSet<String>) {
    let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
        return;
    };
    completed.insert(id.to_owned());
}

fn parse_subagent_tool_use(block: &Value, launched: &mut Vec<Workunit>) {
    let Some(name) = block.get("name").and_then(Value::as_str) else {
        return;
    };
    let Some(id) = block.get("id").and_then(Value::as_str) else {
        return;
    };
    let Some(input) = block.get("input").and_then(Value::as_object) else {
        return;
    };
    if input
        .get("subagent_type")
        .and_then(Value::as_str)
        .is_some_and(|agent_type| agent_type == "custom-advisor")
    {
        return;
    }
    let is_batch = name.contains("_batch_");

    if is_batch {
        let Some(tasks) = input.get("tasks").and_then(Value::as_array) else {
            return;
        };
        for (index, task) in tasks.iter().enumerate() {
            let Some(task) = task.as_object() else {
                continue;
            };
            if task
                .get("subagent_type")
                .and_then(Value::as_str)
                .is_some_and(|agent_type| agent_type == "custom-advisor")
            {
                continue;
            }
            if let Some(model) = task.get("claudex_model").and_then(Value::as_str) {
                launched.push(Workunit {
                    unit_id: format!("{id}:{index}"),
                    group_id: id.to_owned(),
                    model: Some(model.to_owned()),
                });
            }
        }
        return;
    }

    let Some(model) = input.get("claudex_model").and_then(Value::as_str) else {
        return;
    };
    launched.push(Workunit {
        unit_id: id.to_owned(),
        group_id: id.to_owned(),
        model: Some(model.to_owned()),
    });
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
