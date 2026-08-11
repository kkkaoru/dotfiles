use serde_json::Value;

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
        Value::Array(blocks) if !blocks.is_empty() => blocks
            .iter()
            .try_fold(false, |found_notification, block| {
                let text = text_block(block)?;
                if text.trim().is_empty() || is_monitor_hint_text(text) {
                    return Some(found_notification);
                }
                if is_internal_notification_text(text) {
                    return Some(true);
                }
                // A real user instruction after a lifecycle block must remain
                // a normal turn rather than being swallowed as a notification.
                None
            })
            .is_some_and(|found_notification| found_notification),
        _ => false,
    }
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
    let text = text.trim();
    [
        ("<agent-message", "</agent-message>"),
        ("<task-notification", "</task-notification>"),
    ]
    .into_iter()
    .any(|(opening, closing)| {
        let Some(end) = text.find(closing) else {
            return false;
        };
        text.starts_with(opening)
            && (text[end + closing.len()..].trim().is_empty()
                || is_monitor_hint_text(&text[end + closing.len()..]))
    }) || text.starts_with("Another Claude session sent a message")
}

pub(super) fn is_monitor_hint_text(text: &str) -> bool {
    text.trim_start()
        .starts_with("If this event is something the user would act on now")
}
