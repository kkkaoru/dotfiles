use serde_json::Value;

const SYSTEM_NOTIFICATION_PREFIX: &str = "[SYSTEM NOTIFICATION - NOT USER INPUT]";
const SYSTEM_REMINDER_OPEN: &str = "<system-reminder";
const SYSTEM_REMINDER_CLOSE: &str = "</system-reminder>";
const ANOTHER_SESSION_PREFIX: &str = "Another Claude session sent a message";
const MONITOR_HINT_PREFIX: &str = "If this event is something the user would act on now";
const TASK_NOTIFICATION_OPEN: &str = "<task-notification";
const TASK_NOTIFICATION_CLOSE: &str = "</task-notification>";
const TASK_ID_OPEN: &str = "<task-id>";
const TASK_ID_CLOSE: &str = "</task-id>";
const AGENT_TASK_ID_HEX_LENGTH: usize = 16;
const LIFECYCLE_TAGS: [(&str, &str); 2] = [
    ("<agent-message", "</agent-message>"),
    (TASK_NOTIFICATION_OPEN, TASK_NOTIFICATION_CLOSE),
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
    if is_background_bash_completion_content(content) {
        return false;
    }
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
            .is_some_and(|text| {
                !is_background_bash_completion_text(text) && is_internal_notification_text(text)
            })
}

pub(super) fn is_internal_notification_text(text: &str) -> bool {
    let text = strip_leading_wrappers(text.trim());
    has_lifecycle_notification(text) || text.starts_with(ANOTHER_SESSION_PREFIX)
}

fn is_background_bash_completion_content(content: &Value) -> bool {
    match content {
        Value::String(text) => is_background_bash_completion_text(text),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(text_block)
            .any(is_background_bash_completion_text),
        _ => false,
    }
}

fn is_background_bash_completion_text(text: &str) -> bool {
    let Some(body) = lifecycle_body(text, TASK_NOTIFICATION_OPEN, TASK_NOTIFICATION_CLOSE) else {
        return false;
    };
    let Some(task_id) = lifecycle_body(body, TASK_ID_OPEN, TASK_ID_CLOSE) else {
        return false;
    };
    let task_id = task_id.trim();
    !task_id.is_empty() && !is_agent_task_id(task_id)
}

fn lifecycle_body<'a>(text: &'a str, opening: &str, closing: &str) -> Option<&'a str> {
    let start = text.find(opening)?;
    let opening_end = text[start..].find('>')? + start + 1;
    let end = text[opening_end..].find(closing)? + opening_end;
    Some(&text[opening_end..end])
}

fn is_agent_task_id(task_id: &str) -> bool {
    task_id.strip_prefix('a').is_some_and(|suffix| {
        suffix.len() == AGENT_TASK_ID_HEX_LENGTH
            && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
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
