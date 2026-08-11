use serde_json::Value;

use super::super::agent_effort_matching::value_texts;
use super::background_launch;

pub(super) fn active_user_supplied_name(messages: &[Value], name: &str) -> bool {
    let start = messages
        .iter()
        .rposition(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .map_or(0, |index| index + 1);
    messages[start..]
        .iter()
        .rev()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content"))
        .flat_map(value_texts)
        .find(|text| !background_launch::is_hook_or_mailbox_only(text))
        .is_some_and(|text| explicitly_names_agent(text, name))
}

fn explicitly_names_agent(text: &str, name: &str) -> bool {
    [
        format!("`{name}`"),
        format!("\"{name}\""),
        format!("@{name}"),
        format!("name {name}"),
        format!("names {name}"),
        format!("named {name}"),
        format!("named teammate {name}"),
        format!("名前を{name}"),
        format!("{name}という名前"),
    ]
    .iter()
    .any(|pattern| text.contains(pattern))
}
