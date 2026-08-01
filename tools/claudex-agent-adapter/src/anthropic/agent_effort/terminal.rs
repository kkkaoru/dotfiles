use std::collections::HashSet;

use serde_json::Value;

pub(super) fn terminal_task_notification_ids(messages: &[Value]) -> HashSet<String> {
    let mut ids = HashSet::new();
    for message in messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
    {
        let Some(content) = message.get("content") else {
            continue;
        };
        record_terminal_content(content, &mut ids);
    }
    ids
}

fn record_terminal_content(content: &Value, ids: &mut HashSet<String>) {
    match content {
        Value::String(text) => record_terminal_task_notification(text, ids),
        Value::Array(items) => {
            for text in items
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|item| item.get("text").and_then(Value::as_str))
            {
                record_terminal_task_notification(text, ids);
            }
        }
        _ => {}
    }
}

fn record_terminal_task_notification(text: &str, ids: &mut HashSet<String>) {
    let text = text.trim();
    if !text.starts_with("<task-notification>")
        || !matches!(
            xml_tag(text, "status"),
            Some("completed" | "failed" | "stopped")
        )
    {
        return;
    }
    if let Some(tool_use_id) = xml_tag(text, "tool-use-id") {
        ids.insert(tool_use_id.to_owned());
    }
}

fn xml_tag<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let value = text.split_once(&open)?.1.split_once(&close)?.0.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_terminal_statuses_and_rejects_malformed_content() {
        assert!(terminal_task_notification_ids(&[
            json!({"role":"assistant","content":"<task-notification><status>completed</status></task-notification>"})
        ])
        .is_empty());
        let messages = vec![
            json!({"role":"user"}),
            json!({"role":"user","content":null}),
            json!({"role":"user","content":[
                {"type":"other","text":"<task-notification><status>completed</status></task-notification>"},
                {"type":"text","text":"<task-notification><status>unknown</status><tool-use-id>x</tool-use-id></task-notification>"},
                {"type":"text","text":"<task-notification><status>failed</status></task-notification>"},
                {"type":"text","text":"<task-notification><status>stopped</status><tool-use-id>tool-1</tool-use-id></task-notification>"},
                {"type":"text","text":"<task-notification><status>completed</status><tool-use-id> </tool-use-id></task-notification>"}
            ]}),
        ];
        let ids = terminal_task_notification_ids(&messages);
        assert_eq!(ids, HashSet::from(["tool-1".to_owned()]));
    }
}
