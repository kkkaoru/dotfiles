#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::{collections::HashMap, sync::{Arc, Barrier}, thread};

    use super::*;

    fn request(session: &str, messages: Vec<Value>) -> MessagesRequest {
        MessagesRequest {
            model: "main".to_owned(),
            system: Value::String("stable system".to_owned()),
            messages,
            // These tests exercise explicit Agent Teams reuse. Ordinary
            // Agent/Task sessions are covered separately and must not receive
            // mailbox guidance.
            tools: vec![
                json!({"name":"Agent"}),
                json!({"name":"SendMessage"}),
                json!({"name":"TeamSendMessage"}),
            ],
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

    fn launch_with_scope(tool_use_id: &str, recipient: &str, scope: &str, model: &str) -> Vec<Value> {
        vec![
            json!({
                "role":"assistant",
                "content":[{
                    "type":"tool_use",
                    "id":tool_use_id,
                    "name":"Agent",
                    "input":{"prompt":scope,"claudex_model":model}
                }]
            }),
            launch(tool_use_id, recipient),
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
    fn semantic_duplicate_scope_does_not_create_a_second_worker() {
        let registry = SubagentReuseRegistry::default();
        let mut messages = launch_with_scope(
            "tool-a",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        );
        messages.extend(launch_with_scope(
            "tool-b",
            "worker-b",
            "  audit   the rust adapter tests  ",
            "worker-model",
        ));
        let mut request = request("session-a", messages);
        registry.observe_and_restore(&mut request);
        assert_eq!(registry.state_for("session-a"), Some(vec!["worker-a".to_owned()]));
    }

    #[test]
    fn terminal_worker_can_be_relaunched_for_a_new_attempt() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope("tool-a", "worker-a", "Audit the Rust adapter tests", "worker-model"),
        );
        registry.observe_and_restore(&mut first);
        let mut second_messages = launch_with_scope(
            "tool-b",
            "worker-b",
            "Audit the Rust adapter tests",
            "worker-model",
        );
        second_messages.insert(
            0,
            json!({"role":"user","content":"<task-id>worker-a</task-id><status>completed</status>"}),
        );
        let mut second = request("session-a", second_messages);
        registry.observe_and_restore(&mut second);
        assert_eq!(
            registry.state_for("session-a"),
            Some(vec!["worker-a".to_owned(), "worker-b".to_owned()])
        );
    }

    #[test]
    fn active_worker_launch_with_same_scope_does_not_create_duplicate() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope("tool-a", "worker-a", "Audit the Rust adapter tests", "worker-model"),
        );
        registry.observe_and_restore(&mut first);
        let mut second = request(
            "session-a",
            launch_with_scope("tool-b", "worker-b", "Audit the Rust adapter tests", "worker-model"),
        );
        registry.observe_and_restore(&mut second);
        assert_eq!(
            registry.state_for("session-a"),
            Some(vec!["worker-a".to_owned()])
        );
    }

    #[test]
    fn queued_send_message_status_is_preserved_for_reuse() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope("tool-a", "worker-a", "Audit the Rust adapter tests", "worker-model"),
        );
        registry.observe_and_restore(&mut first);
        let mut resumed = request(
            "session-a",
            vec![json!({
                "role":"user",
                "content":[{"type":"tool_result","tool_use_id":"send-a","content":"Agent \"worker-a\" had no active task; resumed from transcript in the background with your message."}]
            })],
        );
        registry.observe_and_restore(&mut resumed);
        assert!(resumed.system.to_string().contains("worker-a"));
        assert!(resumed.system.to_string().contains("message_queued"));
        assert_eq!(registry.state_for("session-a").expect("state").len(), 1);
    }

    #[test]
    fn matching_scope_is_first_in_actual_reuse_guidance() {
        let registry = SubagentReuseRegistry::default();
        let mut initial = request(
            "session-a",
            [
                launch_with_scope("tool-a", "worker-css", "Review CSS layout", "css-model"),
                launch_with_scope("tool-b", "worker-rust", "Audit Rust adapter tests", "rust-model"),
            ]
            .into_iter()
            .flatten()
            .collect(),
        );
        registry.observe_and_restore(&mut initial);
        let mut resumed = request(
            "session-a",
            vec![json!({"role":"user","content":"continue Rust adapter tests"})],
        );
        registry.observe_and_restore(&mut resumed);
        let guidance = resumed.system.to_string();
        assert!(guidance.find("worker-rust").expect("matching worker") < guidance.find("worker-css").expect("unrelated worker"));
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
    fn ordinary_agent_session_does_not_restore_mailbox_guidance() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request("ordinary-session", vec![launch("tool-a", "worker-a")]);
        first.tools = vec![json!({"name":"Agent"}), json!({"name":"SendMessage"})];
        registry.observe_and_restore(&mut first);

        let mut resumed = request(
            "ordinary-session",
            vec![json!({"role":"user","content":"continue"})],
        );
        resumed.tools = vec![json!({"name":"Agent"}), json!({"name":"SendMessage"})];
        registry.observe_and_restore(&mut resumed);
        assert!(!resumed.system.to_string().contains(REUSE_GUIDANCE_MARKER));
    }

    #[test]
    fn explicit_agent_teams_session_restores_mailbox_guidance() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request("team-session", vec![launch("tool-a", "worker-a")]);
        registry.observe_and_restore(&mut first);

        let mut resumed = request(
            "team-session",
            vec![json!({"role":"user","content":"continue"})],
        );
        registry.observe_and_restore(&mut resumed);
        assert!(resumed.system.to_string().contains(REUSE_GUIDANCE_MARKER));
        assert!(agent_teams_enabled(&resumed));
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

    #[test]
    fn concurrent_persistence_does_not_race_the_atomic_replace() {
        let root = tempfile::tempdir().expect("reuse registry fixture");
        let path = root.path().join("reuse.json");
        let store = Arc::new(Store {
            path: path.clone(),
            save_lock: Mutex::new(()),
        });
        let barrier = Arc::new(Barrier::new(16));
        let threads = (0..16)
            .map(|index| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    let mut states = HashMap::new();
                    states.insert(format!("session-{index}"), SessionState::default());
                    store.save(states)
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread
                .join()
                .expect("persistence thread")
                .expect("serialized persistence");
        }
        let bytes = std::fs::read(path).expect("persisted registry");
        serde_json::from_slice::<StoredStates>(&bytes).expect("valid registry JSON");
    }
}
