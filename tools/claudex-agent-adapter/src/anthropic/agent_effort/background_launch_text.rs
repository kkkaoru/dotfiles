use serde_json::Value;

pub(super) fn active_user_text(messages: &[Value]) -> Option<String> {
    let start = messages
        .iter()
        .rposition(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .map_or(0, |index| index + 1);
    let texts = messages[start..]
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(user_message_text)
        .filter(|text| !is_hook_or_mailbox_only(text))
        .collect::<Vec<_>>();
    (!texts.is_empty()).then(|| texts.join("\n"))
}

pub(in crate::anthropic) fn is_hook_or_mailbox_only(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    if text.contains("<agent-message")
        || text.contains("<teammate-message")
        || trimmed.starts_with("Another Claude session sent a message")
    {
        return true;
    }
    // Trailing CC hook / routing reminders must not hide the real user ask.
    let without_reminders = strip_system_reminders(text);
    without_reminders.trim().is_empty()
}

pub(super) fn strip_system_reminders(text: &str) -> String {
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

pub(super) fn user_message_text(message: &Value) -> Option<String> {
    match message.get("content")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}
