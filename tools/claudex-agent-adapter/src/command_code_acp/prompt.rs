use agent_client_protocol as acp;
use serde_json::Value;

pub fn is_command_code_model(model: &str) -> bool {
    let lower = model.trim().to_ascii_lowercase();
    lower.contains("muse-spark") || lower.contains("command-code")
}

pub fn prompt_text(request: &acp::PromptRequest) -> String {
    let Ok(value) = serde_json::to_value(request) else {
        return String::new();
    };
    wrap_one_shot_task(&slim_headless_prompt(&collect_text(&value["prompt"])))
}

fn wrap_one_shot_task(task: &str) -> String {
    let task = task.trim();
    if task.is_empty() {
        return String::new();
    }
    format!(
        "{task}\n\nThis is a new one-shot SubAgent task. Do not greet, recap git status, or ask what to pick up. A short status or phase update is not the answer: use native tools to complete the delegated task before any final assistant text. If the prompt asks for status after each phase, do that only between tool calls, never as the whole reply. Progress is shown automatically via native thinking/? elapsed and web cards; do not print ●, ▶, ✓, ✗, Status:, or still-working lines. Write ordinary findings in assistant text only at the end. Do not print fixed phrases such as ツール結果待ち, 続きの調査または回答, or 次: …. Put the final answer in assistant text, not only thinking."
    )
}

fn is_instruction_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty()
        || trimmed.starts_with("claudex_")
        || trimmed.contains("claudex-routing")
        || trimmed.contains("selected_workers")
        || trimmed.contains("ctx-agent-history-search")
        || trimmed.contains("You are the model inside the Claude Code agent harness")
        || trimmed.contains("You are the model inside Claudex")
        || trimmed.contains("You are a provider-native ACP worker")
        || trimmed.contains("Claudex SubAgent routing on ACP")
        || trimmed.contains("Claudex provider-native ACP")
        || trimmed.contains("Shared-workspace safety is mandatory")
        || trimmed.contains("Command Code Muse Spark worker")
        || trimmed.contains("Ignore Claudex routing tables")
        || trimmed.contains("Continue this Claude Code conversation")
        || trimmed.contains("role-tagged history follows")
}

/// Drop Claude/Claudex routing dumps so Muse Spark only sees the delegated task.
pub fn slim_headless_prompt(prompt: &str) -> String {
    let without_reminders = strip_tagged_blocks(prompt, "<system-reminder>", "</system-reminder>");
    let without_agent_id = strip_tagged_blocks(
        &without_reminders,
        "<claudex-agent-id>",
        "</claudex-agent-id>",
    );
    let slim = without_agent_id
        .lines()
        .filter(|line| !is_instruction_line(line))
        .collect::<Vec<_>>()
        .join("\n");
    let slim = slim.trim();
    if slim.is_empty() {
        return without_agent_id
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                !trimmed.is_empty()
                    && !trimmed.starts_with("claudex_")
                    && !trimmed.contains("Continue this Claude Code conversation")
                    && !trimmed.contains("role-tagged history follows")
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned();
    }
    if slim.len() > 2_000 {
        slim.chars()
            .rev()
            .take(2_000)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    } else {
        slim.to_owned()
    }
}

fn strip_tagged_blocks(input: &str, open: &str, close: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    while let Some(start) = rest.find(open) {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + open.len()..];
        match after_open.find(close) {
            Some(end) => rest = &after_open[end + close.len()..],
            None => {
                // Unclosed tag: keep the remainder so the delegated task is not dropped.
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

fn collect_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(collect_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(object) => object
            .get("text")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| object.get("content").map(collect_text))
            .unwrap_or_default(),
        _ => String::new(),
    }
}
