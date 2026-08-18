use serde_json::Value;

const SYSTEM_NOTIFICATION_PREFIX: &str = "[SYSTEM NOTIFICATION - NOT USER INPUT]";
const SYSTEM_REMINDER_OPEN: &str = "<system-reminder";
const SYSTEM_REMINDER_CLOSE: &str = "</system-reminder>";
const ANOTHER_SESSION_PREFIX: &str = "Another Claude session sent a message";
const MONITOR_HINT_PREFIX: &str = "If this event is something the user would act on now";
const LIFECYCLE_TAGS: [(&str, &str); 2] = [
    ("<agent-message", "</agent-message>"),
    ("<task-notification", "</task-notification>"),
];

pub(super) fn is_empty_user_content(content: &Value) -> bool {
    match content {
        Value::Null => true,
        Value::String(text) => text.trim().is_empty(),
        Value::Array(blocks) => {
            blocks.is_empty()
                || blocks.iter().all(|block| {
                    block
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind == "text")
                        && block
                            .get("text")
                            .and_then(Value::as_str)
                            .is_some_and(|text| text.trim().is_empty())
                })
        }
        _ => false,
    }
}

pub(super) fn is_internal_notification_content(content: &Value) -> bool {
    match content {
        Value::String(text) => is_internal_notification_text(text),
        Value::Array(blocks) if !blocks.is_empty() => {
            notification_blocks_without_instruction(blocks)
        }
        _ => false,
    }
}

fn notification_blocks_without_instruction(blocks: &[Value]) -> bool {
    blocks
        .iter()
        .try_fold(false, |found_notification, block| {
            classify_notification_block(block)
                .map(|is_notification| found_notification || is_notification)
        })
        .is_some_and(|found_notification| found_notification)
}

fn classify_notification_block(block: &Value) -> Option<bool> {
    let text = text_block(block)?;
    if is_ignorable_wrapper_text(text) {
        return Some(false);
    }
    // A real user instruction after a lifecycle block must remain a normal
    // turn rather than being swallowed as a notification.
    is_internal_notification_text(text).then_some(true)
}

pub(super) fn text_block(block: &Value) -> Option<&str> {
    (block.get("type").and_then(Value::as_str) == Some("text"))
        .then(|| block.get("text").and_then(Value::as_str))
        .flatten()
}

pub(super) fn is_internal_text_block(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) == Some("text")
        && block
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(is_internal_notification_text)
}

pub(super) fn is_internal_notification_text(text: &str) -> bool {
    let text = strip_leading_wrappers(text.trim());
    has_lifecycle_notification(text) || text.starts_with(ANOTHER_SESSION_PREFIX)
}

pub(super) fn is_monitor_hint_text(text: &str) -> bool {
    text.trim_start().starts_with(MONITOR_HINT_PREFIX)
}

fn has_lifecycle_notification(text: &str) -> bool {
    LIFECYCLE_TAGS
        .into_iter()
        .any(|(opening, closing)| lifecycle_remainder_is_wrapper(text, opening, closing))
}

fn lifecycle_remainder_is_wrapper(text: &str, opening: &str, closing: &str) -> bool {
    let Some(end) = text.find(closing) else {
        return false;
    };
    text.starts_with(opening) && is_ignorable_wrapper_text(&text[end + closing.len()..])
}

fn is_ignorable_wrapper_text(text: &str) -> bool {
    let stripped = strip_leading_wrappers(text.trim());
    stripped.is_empty() || is_monitor_hint_text(stripped)
}

fn strip_leading_wrappers(text: &str) -> &str {
    let mut rest = text;
    loop {
        let trimmed = rest.trim_start();
        if let Some(after) = trimmed.strip_prefix(SYSTEM_NOTIFICATION_PREFIX) {
            rest = after;
            continue;
        }
        if let Some(after) = strip_leading_system_reminder(trimmed) {
            rest = after;
            continue;
        }
        return trimmed;
    }
}

fn strip_leading_system_reminder(text: &str) -> Option<&str> {
    if !text.starts_with(SYSTEM_REMINDER_OPEN) {
        return None;
    }
    let end = text.find(SYSTEM_REMINDER_CLOSE)?;
    Some(&text[end + SYSTEM_REMINDER_CLOSE.len()..])
}
