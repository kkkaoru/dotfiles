use agent_client_protocol as acp;
use serde_json::Value;

pub fn prompt_text(request: &acp::PromptRequest) -> String {
    let Ok(value) = serde_json::to_value(request) else {
        return String::new();
    };
    collect_text(&value["prompt"])
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
