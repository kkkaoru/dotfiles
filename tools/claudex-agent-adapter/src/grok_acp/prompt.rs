use serde_json::Value;

use super::plugin;

pub(super) fn provider_instructions(params: &Value) -> String {
    let base = params
        .get("baseInstructions")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let adapter = params
        .get("developerInstructions")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let base = base
        .strip_suffix(adapter)
        .unwrap_or(base)
        .trim_end_matches(['\n', ' ']);
    if base.is_empty() {
        return plugin::ROUTING_INSTRUCTIONS.to_owned();
    }
    format!("{base}\n\n{}", plugin::ROUTING_INSTRUCTIONS)
}

pub(super) fn input_text(input: &Value) -> String {
    match input {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("content").and_then(Value::as_str))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        value => value.to_string(),
    }
}

pub(super) fn grok_effort(effort: &str) -> Option<&'static str> {
    match effort {
        "low" => Some("low"),
        "mid" | "medium" => Some("medium"),
        "high" | "xhigh" | "max" => Some("high"),
        _ => None,
    }
}
