use serde_json::{Value, json};

const FALLBACK_AGENT_TOOLS: [&str; 2] = ["Agent", "Task"];

pub(super) fn append_missing_agent_tools(tools: &mut Vec<Value>) {
    if tools.iter().any(|tool| {
        matches!(
            tool.get("name").and_then(Value::as_str),
            Some("Agent" | "Task")
        )
    }) {
        return;
    }
    for name in FALLBACK_AGENT_TOOLS {
        tools.push(json!({
            "name": name,
            "description": format!(
                "Launch one Claude Code SubAgent with the routed claudex model. Use {name} for delegated work; pass the exact claudex_model and claudex_effort fields."
            ),
            "input_schema": {
                "type": "object",
                "properties": {
                    "description": {"type":"string"},
                    "prompt": {"type":"string"},
                    "subagent_type": {"type":"string"},
                    "model": {"type":"string"},
                    "run_in_background": {"type":"boolean"},
                    "resume": {"type":"string"},
                    "name": {"type":"string"}
                },
                "required": ["description", "prompt", "subagent_type"]
            }
        }));
    }
}
