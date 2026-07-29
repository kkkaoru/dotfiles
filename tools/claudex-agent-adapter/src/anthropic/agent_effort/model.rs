use std::collections::BTreeSet;

use serde_json::Value;

use super::ADAPTER_MODEL;

pub(in crate::anthropic) fn is_agent_tool(tool_name: &str) -> bool {
    matches!(tool_name, "Agent" | "Task")
}

pub(in crate::anthropic) fn requested_model(arguments: &Value) -> Option<&str> {
    arguments
        .get(ADAPTER_MODEL)
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
}

pub(in crate::anthropic) fn disabled_subagent_model<'a>(
    tool_name: &str,
    arguments: &'a Value,
    disabled_models: &BTreeSet<String>,
) -> Option<&'a str> {
    is_agent_tool(tool_name)
        .then(|| requested_model(arguments))
        .flatten()
        .filter(|model| disabled_models.contains(*model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_only_disabled_agent_and_task_launch_models() {
        let disabled = BTreeSet::from(["qwen-disabled".to_owned()]);
        let launch = json!({"claudex_model":"qwen-disabled"});

        assert_eq!(
            disabled_subagent_model("Agent", &launch, &disabled),
            Some("qwen-disabled")
        );
        assert_eq!(
            disabled_subagent_model("Task", &launch, &disabled),
            Some("qwen-disabled")
        );
        assert_eq!(
            disabled_subagent_model("WebSearch", &launch, &disabled),
            None
        );
    }
}
