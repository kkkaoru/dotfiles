#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::{
        collections::HashMap,
        sync::{Arc, Barrier},
        thread,
    };

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

    fn launch_with_scope(
        tool_use_id: &str,
        recipient: &str,
        scope: &str,
        model: &str,
    ) -> Vec<Value> {
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
    fn live_agent_task_ids_keep_active_claude_code_ids_only() {
        let mut messages = launch_with_scope(
            "tool-live",
            "a4496564387a2561f",
            "Implement AzooKey pruning fix",
            "worker-model",
        );
        messages.push(launch("tool-name", "worker-a"));
        messages.push(launch("tool-done", "a906c77ad60469b0a"));
        messages.push(json!({
            "role":"user",
            "content":"<task-id>a906c77ad60469b0a</task-id><status>completed</status>"
        }));
        assert_eq!(
            live_agent_task_ids(&messages),
            vec!["a4496564387a2561f".to_owned()]
        );
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
        assert_eq!(
            registry.state_for("session-a"),
            Some(vec!["worker-a".to_owned()])
        );
    }

    #[test]
    fn terminal_worker_can_be_relaunched_for_a_new_attempt() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope(
                "tool-a",
                "worker-a",
                "Audit the Rust adapter tests",
                "worker-model",
            ),
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
            launch_with_scope(
                "tool-a",
                "worker-a",
                "Audit the Rust adapter tests",
                "worker-model",
            ),
        );
        registry.observe_and_restore(&mut first);
        let mut second = request(
            "session-a",
            launch_with_scope(
                "tool-b",
                "worker-b",
                "Audit the Rust adapter tests",
                "worker-model",
            ),
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
            launch_with_scope(
                "tool-a",
                "worker-a",
                "Audit the Rust adapter tests",
                "worker-model",
            ),
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
                launch_with_scope(
                    "tool-b",
                    "worker-rust",
                    "Audit Rust adapter tests",
                    "rust-model",
                ),
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
        assert!(
            guidance.find("worker-rust").expect("matching worker")
                < guidance.find("worker-css").expect("unrelated worker")
        );
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
        let guidance = resumed.system.to_string();
        assert!(guidance.contains(REUSE_GUIDANCE_MARKER));
        assert!(guidance.contains("worker-a"));
        assert!(guidance.contains("resume"));
        assert!(!guidance.contains("TeamSendMessage"));
    }

    fn launch_arguments(prompt: &str, model: &str) -> Value {
        json!({
            "prompt": prompt,
            "claudex_model": model,
            "subagent_type": "claudex-worker",
            "run_in_background": true
        })
    }

    #[test]
    fn reuse_keys_off_user_id_json_session_when_transport_header_is_missing() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request("ignored", vec![launch("tool-a", "worker-a")]);
        first.metadata = json!({"user_id": r#"{"session_id":"from-user"}"#});
        registry.observe_and_restore(&mut first);
        assert_eq!(
            registry.state_for("from-user"),
            Some(vec!["worker-a".to_owned()])
        );
        assert_eq!(registry.state_for("ignored"), None);
    }

    #[test]
    fn concurrent_claude_sessions_do_not_reuse_each_others_workers() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope(
                "tool-a",
                "worker-a",
                "Audit the Rust adapter tests",
                "worker-model",
            ),
        );
        registry.observe_and_restore(&mut first);

        let mut peer = request(
            "session-b",
            vec![json!({"role":"user","content":"independent tui"})],
        );
        registry.observe_and_restore(&mut peer);
        assert!(!peer.system.to_string().contains("worker-a"));
        assert!(
            registry
                .state_for("session-b")
                .unwrap_or_default()
                .is_empty()
        );

        let mut arguments = launch_arguments("Audit the Rust adapter tests", "worker-model");
        assert_eq!(
            registry.rewrite_launch_input("session-b", &mut arguments),
            None,
            "another claudex TUI must not resume this session's SubAgent"
        );
        assert!(arguments.get("resume").is_none());
        assert_eq!(
            registry.rewrite_launch_input("session-a", &mut arguments),
            Some("worker-a".to_owned())
        );
    }

    #[test]
    fn same_scope_active_launch_is_rewritten_to_resume_instead_of_a_new_spawn() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope(
                "tool-a",
                "worker-a",
                "Audit the Rust adapter tests",
                "worker-model",
            ),
        );
        registry.observe_and_restore(&mut first);
        let mut arguments = launch_arguments("Audit the Rust adapter tests", "worker-model");
        assert_eq!(
            registry.rewrite_launch_input("session-a", &mut arguments),
            Some("worker-a".to_owned())
        );
        assert_eq!(arguments["resume"], "worker-a");
        assert_eq!(registry.state_for("session-a").expect("state").len(), 1);
    }

    #[test]
    fn completed_same_scope_worker_is_revived_with_resume_instead_of_a_new_spawn() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope(
                "tool-a",
                "worker-a",
                "Audit the Rust adapter tests",
                "worker-model",
            ),
        );
        registry.observe_and_restore(&mut first);
        let mut completed = request(
            "session-a",
            vec![
                json!({"role":"user","content":"<task-id>worker-a</task-id><status>completed</status>"}),
            ],
        );
        registry.observe_and_restore(&mut completed);
        let mut arguments = launch_arguments("Audit the Rust adapter tests", "worker-model");
        assert_eq!(
            registry.rewrite_launch_input("session-a", &mut arguments),
            Some("worker-a".to_owned())
        );
        assert_eq!(arguments["resume"], "worker-a");
    }

    #[test]
    fn failed_or_stopped_worker_is_not_rewritten_to_resume() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope(
                "tool-a",
                "worker-a",
                "Audit the Rust adapter tests",
                "worker-model",
            ),
        );
        registry.observe_and_restore(&mut first);
        for status in ["failed", "cancelled", "stopped"] {
            let mut terminal = request(
                "session-a",
                vec![
                    json!({"role":"user","content":format!("<task-id>worker-a</task-id><status>{status}</status>")}),
                ],
            );
            registry.observe_and_restore(&mut terminal);
            let mut arguments = launch_arguments("Audit the Rust adapter tests", "worker-model");
            assert_eq!(
                registry.rewrite_launch_input("session-a", &mut arguments),
                None,
                "{status} workers must stay launchable as a fresh spawn"
            );
            assert!(arguments.get("resume").is_none());
        }
    }

    #[test]
    fn independent_scope_still_launches_a_new_worker() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope(
                "tool-a",
                "worker-a",
                "Audit the Rust adapter tests",
                "worker-model",
            ),
        );
        registry.observe_and_restore(&mut first);
        let mut arguments = launch_arguments("Review CSS layout", "worker-model");
        assert_eq!(
            registry.rewrite_launch_input("session-a", &mut arguments),
            None
        );
        assert!(arguments.get("resume").is_none());
    }

    #[test]
    fn explicit_resume_is_left_alone() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope(
                "tool-a",
                "worker-a",
                "Audit the Rust adapter tests",
                "worker-model",
            ),
        );
        registry.observe_and_restore(&mut first);
        let mut arguments = json!({
            "prompt":"Audit the Rust adapter tests",
            "claudex_model":"worker-model",
            "resume":"already-chosen"
        });
        assert_eq!(
            registry.rewrite_launch_input("session-a", &mut arguments),
            None
        );
        assert_eq!(arguments["resume"], "already-chosen");
    }

    #[test]
    fn same_scope_different_model_resumes_existing_worker() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope(
                "tool-a",
                "worker-a",
                "Audit the Rust adapter tests",
                "worker-model",
            ),
        );
        registry.observe_and_restore(&mut first);
        let mut arguments = launch_arguments("Audit the Rust adapter tests", "other-model");
        assert_eq!(
            registry.rewrite_launch_input("session-a", &mut arguments),
            Some("worker-a".to_owned())
        );
        assert_eq!(arguments["resume"], "worker-a");
    }

    #[test]
    fn description_is_preferred_over_prompt_for_same_scope() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            vec![
                json!({
                    "role":"assistant",
                    "content":[{
                        "type":"tool_use",
                        "id":"tool-a",
                        "name":"Agent",
                        "input":{
                            "description":"Reproduce azookey conversion bug",
                            "prompt":"Use gpt to map the conversion pipeline.",
                            "claudex_model":"gpt-test"
                        }
                    }]
                }),
                launch("tool-a", "worker-a"),
            ],
        );
        registry.observe_and_restore(&mut first);
        let mut arguments = json!({
            "description":"Reproduce azookey conversion bug",
            "prompt":"Use command code to map the conversion pipeline.",
            "claudex_model":"command-code"
        });
        assert_eq!(
            registry.rewrite_launch_input("session-a", &mut arguments),
            Some("worker-a".to_owned())
        );
    }

    #[test]
    fn inflight_placeholder_occupies_scope_before_tool_result() {
        let registry = SubagentReuseRegistry::default();
        let arguments = json!({
            "description":"Trace azookey conversion pipeline",
            "prompt":"Start with Vibrato boundaries.",
            "claudex_model":"gpt-test"
        });
        registry.note_inflight_launch("session-a", &arguments, "tool-pending");
        assert!(registry.scope_is_occupied("session-a", &arguments));
        assert!(
            registry
                .rewrite_launch_input("session-a", &mut arguments.clone())
                .is_none()
        );
        assert_eq!(registry.state_for("session-a"), Some(Vec::<String>::new()));
    }

    #[test]
    fn unique_fuzzy_scope_overlap_does_not_rewrite_independent_fanout() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope(
                "tool-a",
                "worker-a",
                "Audit the Rust adapter tests",
                "worker-model",
            ),
        );
        registry.observe_and_restore(&mut first);
        let mut arguments = launch_arguments("continue Rust adapter tests", "worker-model");
        assert_eq!(
            registry.rewrite_launch_input("session-a", &mut arguments),
            None,
            "fuzzy follow-ups stay as new launches; only exact same-scope launches resume"
        );
    }

    #[test]
    fn three_independent_pathspec_scopes_are_not_rewritten_onto_one_worker() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope(
                "tool-a",
                "worker-a",
                "R2 catalog sync full perf",
                "worker-model",
            ),
        );
        registry.observe_and_restore(&mut first);
        for prompt in [
            "Commit only catalog pathspec A",
            "Commit only queue pathspec B",
            "Commit only worker pathspec C",
        ] {
            let mut arguments = launch_arguments(prompt, "worker-model");
            assert_eq!(
                registry.rewrite_launch_input("session-a", &mut arguments),
                None,
                "{prompt} must stay an independent launch"
            );
        }
    }

    #[test]
    fn ambiguous_similar_workers_are_not_guessed() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            [
                launch_with_scope(
                    "tool-a",
                    "worker-a",
                    "Audit Rust adapter tests",
                    "worker-model",
                ),
                launch_with_scope(
                    "tool-b",
                    "worker-b",
                    "Review Rust error handling",
                    "worker-model",
                ),
            ]
            .into_iter()
            .flatten()
            .collect(),
        );
        registry.observe_and_restore(&mut first);
        let mut arguments = launch_arguments("continue Rust work", "worker-model");
        assert_eq!(
            registry.rewrite_launch_input("session-a", &mut arguments),
            None
        );
    }

    #[test]
    fn reuse_disabled_does_not_rewrite_or_restore_guidance() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope(
                "tool-a",
                "worker-a",
                "Audit the Rust adapter tests",
                "worker-model",
            ),
        );
        registry.observe_and_restore_for_test(&mut first, false);
        let mut resumed = request(
            "session-a",
            vec![json!({"role":"user","content":"continue Rust adapter tests"})],
        );
        registry.observe_and_restore_for_test(&mut resumed, false);
        assert!(!resumed.system.to_string().contains(REUSE_GUIDANCE_MARKER));
        let mut arguments = launch_arguments("Audit the Rust adapter tests", "worker-model");
        assert_eq!(
            registry.rewrite_launch_input_for_test("session-a", &mut arguments, false),
            None
        );
    }

    #[test]
    fn resume_of_completed_worker_does_not_increment_spawn_count() {
        let registry = SubagentReuseRegistry::default();
        let mut first = request(
            "session-a",
            launch_with_scope(
                "tool-a",
                "worker-a",
                "Audit the Rust adapter tests",
                "worker-model",
            ),
        );
        registry.observe_and_restore(&mut first);
        let mut messages = vec![json!({
            "role":"user",
            "content":"<task-id>worker-a</task-id><status>completed</status>"
        })];
        messages.extend(launch_with_scope(
            "tool-c",
            "worker-a",
            "Audit the Rust adapter tests",
            "worker-model",
        ));
        let mut resumed = request("session-a", messages);
        registry.observe_and_restore(&mut resumed);
        assert_eq!(
            registry.state_for("session-a"),
            Some(vec!["worker-a".to_owned()])
        );
        assert_eq!(
            registry.status_for("session-a", "worker-a").as_deref(),
            Some("active")
        );
    }

    #[test]
    fn named_agent_input_alone_does_not_enable_agent_teams_mailbox() {
        let mut ordinary = request(
            "ordinary-named-worker",
            vec![json!({
                "role":"assistant",
                "content":[{
                    "type":"tool_use",
                    "id":"agent-call",
                    "name":"Agent",
                    "input":{"name":"research-worker","run_in_background":true}
                }]
            })],
        );
        ordinary.tools = vec![json!({"name":"Agent"}), json!({"name":"SendMessage"})];
        assert!(!agent_teams_enabled(&ordinary));
        let (tools, _, _) = crate::anthropic::session::tool_configuration(&ordinary, None, None);
        assert!(
            tools.iter().all(|tool| {
                tool.get("name").and_then(Value::as_str) != Some("cc_SendMessage_1")
            })
        );
    }

    #[test]
    fn generic_agent_teams_documentation_does_not_enable_mailbox_transport() {
        let mut ordinary = request(
            "ordinary-documentation",
            vec![json!({
                "role":"user",
                "content":"The Agent Teams documentation is present, but no team was requested."
            })],
        );
        ordinary.tools = vec![json!({"name":"Agent"}), json!({"name":"SendMessage"})];
        assert!(!agent_teams_enabled(&ordinary));
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
