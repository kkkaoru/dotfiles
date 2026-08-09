use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::anthropic::{MessagesRequest, agent_tool_round_ids, exact_async_launch_acknowledgement};

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

fn legacy_cc_agent_round_ids(message: &Value) -> Option<Vec<String>> {
    let ids = message
        .get("content")?
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter(|block| {
            block
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.starts_with("cc_Agent_") && !name.contains("_batch_"))
        })
        .map(|block| {
            block
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()?;
    (!ids.is_empty()).then_some(ids)
}

fn record_message_content(
    content: &[Value],
    launched: &mut Vec<Workunit>,
    completed: &mut HashSet<String>,
    exact_async_acknowledgement: bool,
) {
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => parse_subagent_tool_use(block, launched),
            Some("tool_result") => {
                record_completed_tool(block, completed, exact_async_acknowledgement);
            }
            _ => {}
        }
    }
}

fn record_completed_tool(
    block: &Value,
    completed: &mut HashSet<String>,
    exact_async_acknowledgement: bool,
) {
    if exact_async_acknowledgement {
        return;
    }
    let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
        return;
    };
    completed.insert(id.to_owned());
}

fn record_task_notification(message: &Value, completed: &mut HashSet<String>) {
    let Some(content) = message.get("content") else {
        return;
    };
    match content {
        Value::String(text) => record_task_notification_text(text, completed),
        Value::Array(items) => {
            for text in items
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|item| item.get("text").and_then(Value::as_str))
            {
                record_task_notification_text(text, completed);
            }
        }
        _ => {}
    }
}

fn record_task_notification_text(text: &str, completed: &mut HashSet<String>) {
    let text = text.trim();
    if !text.starts_with("<task-notification>")
        || !["completed", "failed", "stopped"]
            .iter()
            .any(|status| text.contains(&format!("<status>{status}</status>")))
    {
        return;
    }
    if let Some(tool_use_id) = xml_tag(text, "tool-use-id") {
        completed.insert(tool_use_id.to_owned());
    }
}

fn xml_tag<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let value = text.split_once(&open)?.1.split_once(&close)?.0.trim();
    (!value.is_empty()).then_some(value)
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
        launched.extend(
            tasks
                .iter()
                .enumerate()
                .filter_map(|(index, task)| batch_workunit(id, index, task)),
        );
        return;
    }

    let Some(model) = input.get("claudex_model").and_then(Value::as_str) else {
        return;
    };
    // A retry or a second model can receive a fresh tool_use id while still
    // representing the same scope.  Count that scope once; explicit batches
    // above intentionally retain one lane per task because their array length
    // is the caller's explicit launch count.
    let unit_id = input
        .get("prompt")
        .or_else(|| input.get("description"))
        .and_then(Value::as_str)
        .map(normalize_scope)
        .filter(|scope| !scope.is_empty())
        .map(|scope| format!("scope:{scope}"))
        .unwrap_or_else(|| id.to_owned());
    launched.push(Workunit {
        unit_id,
        group_id: id.to_owned(),
        model: Some(model.to_owned()),
    });
}

fn batch_workunit(id: &str, index: usize, task: &Value) -> Option<Workunit> {
    let task = task.as_object()?;
    if task
        .get("subagent_type")
        .and_then(Value::as_str)
        .is_some_and(|agent_type| agent_type == "custom-advisor")
    {
        return None;
    }
    let model = task.get("claudex_model").and_then(Value::as_str)?;
    Some(Workunit {
        unit_id: format!("{id}:{index}"),
        group_id: id.to_owned(),
        model: Some(model.to_owned()),
    })
}

fn normalize_scope(prompt: &str) -> String {
    let mut normalized = String::with_capacity(prompt.len());
    for line in prompt.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("claudex_launch_id:") || trimmed.starts_with("claudex_model:") {
            continue;
        }
        let cleaned = remove_correlation_tag(trimmed);
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(cleaned.trim());
    }
    normalized
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn remove_correlation_tag(text: &str) -> String {
    const OPEN: &str = "<claudex-agent-id>";
    const CLOSE: &str = "</claudex-agent-id>";
    let mut remaining = text;
    let mut cleaned = String::with_capacity(text.len());
    while let Some(start) = remaining.find(OPEN) {
        cleaned.push_str(&remaining[..start]);
        let after_open = &remaining[start + OPEN.len()..];
        let Some(end) = after_open.find(CLOSE) else {
            break;
        };
        remaining = &after_open[end + CLOSE.len()..];
    }
    cleaned.push_str(remaining);
    cleaned
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
