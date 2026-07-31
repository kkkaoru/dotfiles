//! Distinguish follow-up delegation from provider-side thinking.
//!
//! Status is enabled only after a real prior Agent/Task/SendMessage tool call.

use serde_json::Value;

pub(super) const NO_ACTION_NOTICE: &str =
    "SubAgent status: no Agent/Task launch or SendMessage reuse was emitted for this follow-up.";

#[derive(Default)]
pub(super) struct SubagentVisibility {
    initialized: bool,
    report_follow_up: bool,
    saw_action: bool,
}

impl SubagentVisibility {
    pub(super) fn observe_context(&mut self, previous: &[Value], current: &[Value]) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.report_follow_up = latest_user_has_direct_instruction(current)
            && (contains_subagent_action(previous) || contains_subagent_action(current));
    }

    pub(super) fn action_status(&mut self, name: &str, input: &Value) -> Option<String> {
        let action = match name {
            "Agent" | "Task" => launch_status(name, input),
            "SendMessage" => reuse_status(input),
            _ => return None,
        };
        self.saw_action = true;
        self.report_follow_up.then_some(action)
    }

    pub(super) fn no_action_notice(&self) -> Option<&'static str> {
        (self.report_follow_up && !self.saw_action).then_some(NO_ACTION_NOTICE)
    }
}

fn launch_status(name: &str, input: &Value) -> String {
    let worker = short_field(input, &["subagent_type", "description"]).unwrap_or("worker");
    let model = short_field(input, &["claudex_model"]);
    match model {
        Some(model) => format!(
            "SubAgent status: {name} launch emitted for {worker} on {model}; completion is not implied."
        ),
        None => format!(
            "SubAgent status: {name} launch emitted for {worker}; completion is not implied."
        ),
    }
}

fn reuse_status(input: &Value) -> String {
    let recipient = short_field(input, &["to", "recipient", "agent_id"]).unwrap_or("worker");
    format!(
        "SubAgent status: SendMessage reuse emitted for {recipient}; delivery is not completion."
    )
}

fn short_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 120 && !value.contains(['\n', '\r']))
}

fn contains_subagent_action(messages: &[Value]) -> bool {
    messages.iter().any(|message| {
        message.get("role").and_then(Value::as_str) == Some("assistant")
            && message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(is_subagent_action)
    })
}

fn is_subagent_action(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) == Some("tool_use")
        && matches!(
            block.get("name").and_then(Value::as_str),
            Some("Agent" | "Task" | "SendMessage")
        )
}

fn latest_user_has_direct_instruction(messages: &[Value]) -> bool {
    messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|message| message.get("content"))
        .is_some_and(direct_instruction_content)
}

fn direct_instruction_content(content: &Value) -> bool {
    match content {
        Value::String(text) => is_direct_instruction(text),
        Value::Array(blocks) => blocks.iter().any(|block| {
            block.get("type").and_then(Value::as_str) == Some("text")
                && block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(is_direct_instruction)
        }),
        _ => false,
    }
}

fn is_direct_instruction(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && ![
            "<agent-message",
            "<task-notification",
            "<teammate-message",
            "<system-reminder",
            "Another Claude session sent a message",
            "Claudex routing for this turn:",
            "Runtime parallel contract",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reports_only_a_direct_follow_up_after_real_subagent_activity() {
        let history = [json!({
            "role":"assistant",
            "content":[{"type":"tool_use","name":"Agent","id":"toolu_agent","input":{}}]
        })];
        let follow_up = [json!({"role":"user","content":"continue with the next change"})];
        let tool_result = [json!({"role":"user","content":[{
            "type":"tool_result", "tool_use_id":"toolu_agent", "content":"done"
        }]})];
        let notification = [json!({"role":"user","content":"<task-notification>done"})];
        let routed_notification = [json!({"role":"user","content":[
            {"type":"text","text":"<task-notification>done"},
            {"type":"text","text":"Claudex routing for this turn: {}"}
        ]})];

        let mut visible = SubagentVisibility::default();
        visible.observe_context(&history, &follow_up);
        assert_eq!(visible.no_action_notice(), Some(NO_ACTION_NOTICE));

        for current in [
            &tool_result[..],
            &notification[..],
            &routed_notification[..],
        ] {
            let mut hidden = SubagentVisibility::default();
            hidden.observe_context(&history, current);
            assert_eq!(hidden.no_action_notice(), None);
        }
    }

    #[test]
    fn distinguishes_launch_reuse_and_non_subagent_tools() {
        let history = [json!({
            "role":"assistant",
            "content":[{"type":"tool_use","name":"Task","id":"task","input":{}}]
        })];
        let follow_up = [json!({"role":"user","content":"continue"})];
        let mut launch = SubagentVisibility::default();
        launch.observe_context(&history, &follow_up);
        assert!(
            launch
                .action_status(
                    "Agent",
                    &json!({"subagent_type":"claudex-gpt","claudex_model":"gpt-5.6-luna"})
                )
                .is_some_and(|status| status.contains("Agent launch emitted"))
        );
        assert_eq!(launch.no_action_notice(), None);

        let mut reuse = SubagentVisibility::default();
        reuse.observe_context(&history, &follow_up);
        assert!(
            reuse
                .action_status("SendMessage", &json!({"to":"worker-1"}))
                .is_some_and(|status| status.contains("worker-1"))
        );
        assert_eq!(reuse.action_status("Bash", &json!({})), None);
    }
}
