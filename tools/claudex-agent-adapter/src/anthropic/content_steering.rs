use serde_json::Value;

/// Claude Code may put mid-turn steering beside `tool_result` blocks, or in a
/// following text-only user message after those results. Fold either shape.
pub(in crate::anthropic) fn mid_turn_user_steering(messages: &[Value]) -> Option<String> {
    let suffix = super::trailing_user_messages(messages);
    let has_tool_result = suffix.iter().any(|message| {
        message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
            })
    });
    if !has_tool_result {
        return None;
    }
    let text = suffix
        .iter()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .filter_map(super::text_block)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !steering_noise(line))
        .map(extract_steering_body)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn steering_noise(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.contains("<agent-message")
        || trimmed.contains("<teammate-message")
        || trimmed.starts_with("Another Claude session sent a message")
    {
        return true;
    }
    strip_system_reminders(trimmed).trim().is_empty()
}

fn strip_system_reminders(text: &str) -> String {
    let mut rest = text.to_owned();
    while let Some(start) = rest.find("<system-reminder>") {
        let Some(end_rel) = rest[start..].find("</system-reminder>") else {
            break;
        };
        let end = start + end_rel + "</system-reminder>".len();
        rest.replace_range(start..end, " ");
    }
    rest
}

fn extract_steering_body(text: &str) -> String {
    const PREFIX: &str = "The user sent a new message while you were working:";
    const SUFFIX: &str = "Address the message above as you continue this turn.";
    let cleaned = strip_system_reminders(text);
    let body = cleaned
        .trim()
        .strip_prefix(PREFIX)
        .map(str::trim)
        .unwrap_or_else(|| cleaned.trim());
    body.strip_suffix(SUFFIX)
        .map(str::trim)
        .unwrap_or(body)
        .to_owned()
}
