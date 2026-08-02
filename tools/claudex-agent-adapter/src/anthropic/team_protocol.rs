use std::borrow::Cow;

use serde_json::Value;

use super::{MessagesRequest, agent_effort::is_agent_tool};

const GUIDANCE: &str = "Determine SubAgent lifecycle from each Agent or Task tool result, because a named SubAgent may be either a persistent mailbox teammate or a regular background agent. A result containing teammate_spawned or saying that the agent receives instructions via mailbox identifies a teammate: never pass that named teammate's name or name@session agent ID to TaskOutput or TaskList; use SendMessage with the teammate name only when further communication is necessary, otherwise end the turn and wait for Claude Code's automatic teammate message. A result saying Async agent launched identifies a regular background agent: wait for Claude Code's automatic completion notification and follow the recipient ID stated by that result if communication is necessary. Never restart a completed agent merely to collect output. Use TaskOutput only when a tool result explicitly returns a task_id for TaskOutput, never with a display name or agent_id. TaskStop/Stop Task is best-effort and idempotent: invoke it only with the exact active task_id returned by the current Agent/Task result; never guess, reuse, or stop a display name, mailbox agent_id, completion notification, or already-consumed task. If Claude Code reports `No task found` for a stop request, treat the task as already completed or stopped, do not retry, and continue the remaining work.";

const RESULT_CLARIFICATION: &str = "Claudex protocol: this is a named mailbox teammate, not a TaskOutput or TaskList task. Do not pass its name or agent_id to TaskOutput. Use SendMessage with the teammate name when needed, then end the turn and wait for automatic teammate messages. Do not call TaskStop/Stop Task for this mailbox result; if a stale stop reports `No task found`, treat it as an idempotent already-stopped outcome and continue.";

pub(super) fn guidance(request: &MessagesRequest) -> Option<&'static str> {
    let named_agent = request.tools.iter().any(|tool| {
        tool.get("name")
            .and_then(Value::as_str)
            .is_some_and(is_agent_tool)
            && tool.pointer("/input_schema/properties/name").is_some()
    });
    let send_message = request
        .tools
        .iter()
        .any(|tool| tool.get("name").and_then(Value::as_str) == Some("SendMessage"));
    let explicit_team = request.tools.iter().any(|tool| {
        let name = tool.get("name").and_then(Value::as_str).unwrap_or_default();
        matches!(
            name,
            "TeamCreate"
                | "TeamDelete"
                | "TeamList"
                | "TeamRename"
                | "TeamUpdate"
                | "TeamSendMessage"
        ) || name.starts_with("team_")
    }) || request.messages.iter().any(|message| {
        let text = message.to_string();
        text.contains("USE_NAMED_TEAM_MAILBOX") || text.contains("<teammate-message")
    });
    (named_agent && send_message && explicit_team).then_some(GUIDANCE)
}

pub(super) fn clarify_result(text: &str) -> Cow<'_, str> {
    if !is_teammate_result(text) {
        return Cow::Borrowed(text);
    }
    if text.contains(RESULT_CLARIFICATION) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(format!("{text}\n\n{RESULT_CLARIFICATION}"))
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

    use super::{MessagesRequest, RESULT_CLARIFICATION, clarify_result, guidance};

    fn request(tools: Vec<serde_json::Value>, messages: Vec<serde_json::Value>) -> MessagesRequest {
        MessagesRequest {
            model: "main".to_owned(),
            system: serde_json::Value::Null,
            messages,
            tools,
            stream: false,
            output_config: serde_json::Value::Null,
            metadata: serde_json::Value::Null,
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        }
    }

    #[test]
    fn enables_guidance_only_for_named_agents_with_mailbox_tooling() {
        for tool_name in ["Agent", "Task"] {
            let agent = json!({
                "name":tool_name,
                "input_schema":{"properties":{"name":{"type":"string"}}}
            });
            let send = json!({"name":"SendMessage"});
            let text = guidance(&request(
                vec![agent.clone(), send],
                vec![json!({"role":"user","content":"USE_NAMED_TEAM_MAILBOX"})],
            ))
            .expect("team guidance");
            assert!(text.contains("never pass that named teammate's name"));
            assert!(text.contains("Async agent launched"));
            assert!(text.contains("TaskStop/Stop Task is best-effort and idempotent"));
            assert!(text.contains("No task found"));
            assert!(guidance(&request(vec![agent], Vec::new())).is_none());
        }
        assert!(guidance(&request(vec![json!({"name":"Agent"})], Vec::new())).is_none());
        assert!(guidance(&request(vec![json!({"name":"Task"})], Vec::new())).is_none());
        assert!(guidance(&request(vec![json!({"name":"SendMessage"})], Vec::new())).is_none());
        assert!(
            guidance(&request(
                vec![json!({"name":"Read"}), json!({"name":"SendMessage"})],
                Vec::new(),
            ))
            .is_none()
        );
    }

    #[test]
    fn ordinary_named_agent_schema_does_not_enable_mailbox_guidance() {
        let request = request(
            vec![
                json!({
                    "name":"Agent",
                    "input_schema":{"properties":{"name":{"type":"string"}}}
                }),
                json!({"name":"SendMessage"}),
            ],
            vec![json!({"role":"user","content":"launch a regular background worker"})],
        );
        assert!(guidance(&request).is_none());
    }

    #[test]
    fn generic_agent_teams_documentation_does_not_enable_mailbox_guidance() {
        let request = request(
            vec![
                json!({
                    "name":"Agent",
                    "input_schema":{"properties":{"name":{"type":"string"}}}
                }),
                json!({"name":"SendMessage"}),
            ],
            vec![json!({
                "role":"user",
                "content":"The Agent Teams documentation is available for explicit team sessions."
            })],
        );
        assert!(guidance(&request).is_none());
    }

    #[test]
    fn clarifies_mailbox_results_without_changing_original_metadata() {
        let original = "Spawned successfully.\nagent_id: company-profile@session-123\nname: company-profile\nThe agent is now running and will receive instructions via mailbox.";
        let clarified = clarify_result(original);
        assert!(clarified.starts_with(original));
        assert!(clarified.contains(RESULT_CLARIFICATION));
        assert!(clarified.contains("No task found"));
        assert!(clarified.contains("company-profile@session-123"));
        assert_eq!(clarify_result(&clarified), clarified);
        let already_clarified = format!("teammate_spawned {RESULT_CLARIFICATION}");
        assert_eq!(clarify_result(&already_clarified), already_clarified);
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
