#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request(session: &str, messages: Vec<Value>) -> MessagesRequest {
        MessagesRequest {
            model: "main".to_owned(),
            system: Value::String("stable system".to_owned()),
            messages,
            tools: vec![json!({"name":"Agent"}), json!({"name":"SendMessage"})],
            stream: false,
            output_config: Value::Null,
            metadata: json!({
                "_claudex_transport_identity":{"session_id":session}
            }),
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        }
    }

    fn launch(tool_use_id: &str, recipient: &str) -> Value {
        json!({
            "role":"user",
            "content":[{
                "type":"tool_result",
                "tool_use_id":tool_use_id,
                "content":[{"type":"text","text":format!("Async agent launched successfully.\\nagentId: {recipient}")}]
            }]
        })
    }

    fn launch_with_context(tool_use_id: &str, recipient: &str) -> Vec<Value> {
        vec![
            json!({
                "role":"assistant",
                "content":[{
                    "type":"tool_use",
                    "id":tool_use_id,
                    "name":"Agent",
                    "input":{
                        "prompt":"Audit Rust adapter tests and preserve the active worker",
                        "claudex_model":"worker-model"
                    }
                }]
            }),
            launch(tool_use_id, recipient),
            json!({
                "role":"user",
                "content":"<task-id>worker-a</task-id><status>completed</status>"
            }),
        ]
    }

    #[test]
    fn records_recipients_and_persists_across_registry_restart() {
        let root = tempfile::tempdir().expect("reuse registry fixture");
        let path = root.path().join("reuse.json");
        let registry = SubagentReuseRegistry::with_store(path.clone());
        let mut first = request("session-a", vec![launch("tool-a", "worker-a")]);
        registry.observe_and_restore(&mut first);
        assert_eq!(
            registry.state_for("session-a"),
            Some(vec!["worker-a".to_owned()])
        );

        let restored = SubagentReuseRegistry::with_store(path);
        let mut resumed = request(
            "session-a",
            vec![json!({"role":"user","content":"compact summary"})],
        );
        restored.observe_and_restore(&mut resumed);
        assert!(resumed.system.to_string().contains(REUSE_GUIDANCE_MARKER));
        assert!(resumed.system.to_string().contains("worker-a"));
    }

    #[test]
    fn duplicate_history_does_not_inflate_cumulative_spawn_count() {
        let registry = SubagentReuseRegistry::default();
        let mut request = request(
            "session-a",
            vec![launch("tool-a", "worker-a"), launch("tool-a", "worker-a")],
        );
        registry.observe_and_restore(&mut request);
        assert_eq!(registry.state_for("session-a").expect("state").len(), 1);
    }

    #[test]
    fn restores_scope_model_and_status_for_dynamic_recipient_assignment() {
        let root = tempfile::tempdir().expect("reuse registry fixture");
        let path = root.path().join("reuse.json");
        let registry = SubagentReuseRegistry::with_store(path.clone());
        let mut first = request("session-a", launch_with_context("tool-a", "worker-a"));
        registry.observe_and_restore(&mut first);

        let restored = SubagentReuseRegistry::with_store(path);
        let mut resumed = request(
            "session-a",
            vec![json!({"role":"user","content":"continue Rust adapter tests"})],
        );
        restored.observe_and_restore(&mut resumed);
        let guidance = resumed.system.to_string();
        assert!(guidance.contains("worker-a"));
        assert!(guidance.contains("Audit Rust adapter tests"));
        assert!(guidance.contains("worker-model"));
        assert!(guidance.contains("completed"));
    }

    #[test]
    fn launch_tools_are_hidden_only_after_the_session_budget_is_reached() {
        let mut below = request("session-a", Vec::new());
        set_limit_metadata(&mut below, false);
        assert!(should_expose_launch_tools(&below));
        let mut reached = request("session-a", Vec::new());
        set_limit_metadata(&mut reached, true);
        assert!(!should_expose_launch_tools(&reached));
        assert_eq!(DEFAULT_MAX_SUBAGENTS_PER_SESSION, 1_024);
    }

    #[test]
    fn only_native_launch_results_are_recorded() {
        let request = request(
            "session-a",
            vec![json!({
                "role":"user",
                "content":[{"type":"tool_result","tool_use_id":"read","content":"agentId: not-a-launch"}]
            })],
        );
        assert!(launch_records(&request.messages).is_empty());
    }

    #[test]
    fn scope_similarity_prioritizes_the_matching_worker() {
        assert!(
            scope_similarity("audit Rust adapter tests", "continue Rust tests")
                > scope_similarity("review CSS layout", "continue Rust tests")
        );
    }
}
