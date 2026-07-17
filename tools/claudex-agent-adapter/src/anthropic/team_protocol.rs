use std::borrow::Cow;

use serde_json::Value;

const GUIDANCE: &str = "Determine Agent lifecycle from each Agent tool result, because a named Agent may be either a persistent mailbox teammate or a regular background agent. A result containing teammate_spawned or saying that the agent receives instructions via mailbox identifies a teammate: never pass that named teammate's name or name@session agent ID to TaskOutput or TaskList; use SendMessage with the teammate name only when further communication is necessary, otherwise end the turn and wait for Claude Code's automatic teammate message. A result saying Async agent launched identifies a regular background agent: wait for Claude Code's automatic completion notification and follow the recipient ID stated by that result if communication is necessary. Never restart a completed agent merely to collect output. Use TaskOutput only when a tool result explicitly returns a task_id for TaskOutput, never with a display name or agent_id.";

const RESULT_CLARIFICATION: &str = "Claudex protocol: this is a named mailbox teammate, not a TaskOutput or TaskList task. Do not pass its name or agent_id to TaskOutput. Use SendMessage with the teammate name when needed, then end the turn and wait for automatic teammate messages.";

pub(super) fn guidance(tools: &[Value]) -> Option<&'static str> {
    let named_agent = tools.iter().any(|tool| {
        tool.get("name").and_then(Value::as_str) == Some("Agent")
            && tool.pointer("/input_schema/properties/name").is_some()
    });
    let send_message = tools
        .iter()
        .any(|tool| tool.get("name").and_then(Value::as_str) == Some("SendMessage"));
    (named_agent && send_message).then_some(GUIDANCE)
}

pub(super) fn clarify_result(text: &str) -> Cow<'_, str> {
    if is_teammate_result(text) && !text.contains(RESULT_CLARIFICATION) {
        return Cow::Owned(format!("{text}\n\n{RESULT_CLARIFICATION}"));
    }
    Cow::Borrowed(text)
}

fn is_teammate_result(text: &str) -> bool {
    text.contains("teammate_spawned")
        || text.contains("receive instructions via mailbox")
        || (text.contains("agent_id:") && text.contains("name:") && text.contains("mailbox"))
}

#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::{RESULT_CLARIFICATION, clarify_result, guidance};

    #[test]
    fn enables_guidance_only_for_named_agents_with_mailbox_tooling() {
        let agent = json!({
            "name":"Agent",
            "input_schema":{"properties":{"name":{"type":"string"}}}
        });
        let send = json!({"name":"SendMessage"});
        let text = guidance(&[agent.clone(), send]).expect("team guidance");
        assert!(text.contains("never pass that named teammate's name"));
        assert!(text.contains("Async agent launched"));
        assert!(guidance(&[agent]).is_none());
        assert!(guidance(&[json!({"name":"Agent"})]).is_none());
        assert!(guidance(&[json!({"name":"SendMessage"})]).is_none());
        assert!(guidance(&[json!({"name":"Read"}), json!({"name":"SendMessage"})]).is_none());
    }

    #[test]
    fn clarifies_mailbox_results_without_changing_original_metadata() {
        let original = "Spawned successfully.\nagent_id: company-profile@session-123\nname: company-profile\nThe agent is now running and will receive instructions via mailbox.";
        let clarified = clarify_result(original);
        assert!(clarified.starts_with(original));
        assert!(clarified.contains(RESULT_CLARIFICATION));
        assert!(clarified.contains("company-profile@session-123"));
        assert_eq!(clarify_result(&clarified), clarified);
        assert_eq!(
            clarify_result("ordinary tool output"),
            "ordinary tool output"
        );
    }

    #[test]
    fn recognizes_structured_teammate_status() {
        let text = r#"{"status":"teammate_spawned","agent_id":"profile@session"}"#;
        assert!(clarify_result(text).contains(RESULT_CLARIFICATION));
        assert_eq!(
            clarify_result("agent_id: profile\nname: profile"),
            "agent_id: profile\nname: profile"
        );
        assert_eq!(
            clarify_result("name: profile\nmailbox"),
            "name: profile\nmailbox"
        );
        assert!(
            clarify_result("agent_id: profile\nname: profile\nmailbox")
                .contains(RESULT_CLARIFICATION)
        );
    }
}
