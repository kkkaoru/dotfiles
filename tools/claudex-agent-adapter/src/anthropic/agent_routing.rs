use serde_json::Value;

use super::subscription::valid_effort;

const ADAPTER_EFFORT: &str = "claudex_effort";
const ADAPTER_MODEL: &str = "claudex_model";

pub(super) fn hydrate_routing_fields(arguments: &mut Value) {
    let model = arguments
        .get(ADAPTER_MODEL)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| provider_model(arguments))
        .or_else(|| prompt_routing_value(arguments, ADAPTER_MODEL));
    let effort = arguments
        .get(ADAPTER_EFFORT)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| prompt_routing_value(arguments, ADAPTER_EFFORT));
    let Some(object) = arguments.as_object_mut() else {
        return;
    };
    if let Some(model) = model {
        object.insert(ADAPTER_MODEL.to_owned(), Value::String(model));
    }
    if let Some(effort) = effort.filter(|value| valid_effort(value)) {
        object.insert(ADAPTER_EFFORT.to_owned(), Value::String(effort));
    }
}

fn provider_model(arguments: &Value) -> Option<String> {
    arguments
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| model.starts_with("gpt") || model.starts_with("grok"))
        .map(str::to_owned)
}

fn prompt_routing_value(arguments: &Value, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    arguments
        .get("prompt")
        .and_then(Value::as_str)?
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
