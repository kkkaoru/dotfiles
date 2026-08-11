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
