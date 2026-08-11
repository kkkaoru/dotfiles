use serde_json::Value;

use super::Workunit;

pub(in crate::parallel_scheduler::core) fn parse_subagent_tool_use(block: &Value, launched: &mut Vec<Workunit>) {
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

pub(super) fn batch_workunit(id: &str, index: usize, task: &Value) -> Option<Workunit> {
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

pub(super) fn normalize_scope(prompt: &str) -> String {
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

pub(super) fn remove_correlation_tag(text: &str) -> String {
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
