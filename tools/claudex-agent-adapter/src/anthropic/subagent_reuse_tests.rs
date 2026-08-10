#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::{
        collections::HashMap,
        fs,
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
        let mut null_metadata = request("session-a", Vec::new());
        null_metadata.metadata = Value::Null;
        set_limit_metadata(&mut null_metadata, true);
        assert!(null_metadata.metadata.is_object());
    }

    #[test]
    fn empty_ids_and_corrupt_store_do_not_rewrite_or_occupy_scope() {
        let registry = SubagentReuseRegistry::default();
        let mut arguments = launch_arguments("Audit the Rust adapter tests", "worker-model");
        assert_eq!(registry.rewrite_launch_input("", &mut arguments), None);
        assert!(!registry.scope_is_occupied("", &arguments));
        assert!(!registry.scope_is_occupied("session-a", &json!({})));
        registry.note_inflight_launch("", &arguments, "tool-a");
        registry.note_inflight_launch("session-a", &arguments, "");
        registry.note_inflight_launch("session-a", &json!({}), "tool-a");
        assert!(!registry.scope_is_occupied("session-a", &arguments));

        let root = tempfile::tempdir().expect("reuse store fixture");
        let corrupt = root.path().join("corrupt.json");
        fs::write(&corrupt, "{not json").expect("corrupt cache");
        let _ignored = SubagentReuseRegistry::with_store(corrupt);

        let incompatible = root.path().join("old.json");
        fs::write(&incompatible, r#"{"version":0,"sessions":{}}"#).expect("old cache");
        let _ignored = SubagentReuseRegistry::with_store(incompatible);

        let not_a_dir = root.path().join("not-a-dir");
        fs::write(&not_a_dir, "x").expect("file where directory should be");
        let failing = SubagentReuseRegistry::with_store(not_a_dir.join("cache.json"));
        let mut first = request("session-a", launch_with_context("tool-a", "worker-a"));
        failing.observe_and_restore(&mut first);
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
    fn launch_records_cover_empty_scope_and_background_spawn_text() {
        assert!(summarize_scope(&json!({})).is_empty());
        assert!(
            summarize_scope(
                &json!({"prompt":"\nclaudex_hidden\n<claudex-note>skip</claudex-note>\n"})
            )
            .is_empty()
        );
        assert!(!already_has_resume(&json!({})));
        assert!(!already_has_resume(&json!({"resume":""})));
        assert!(already_has_resume(&json!({"resume":"session-1"})));
        assert!(find_reusable_launch(&[], &json!({})).is_none());
        assert!(!scope_is_occupied(&[], ""));

        let records = launch_records(&[
            json!({"role":"assistant"}),
            json!({
                "role":"assistant",
                "content":[{
                    "type":"tool_use",
                    "id":"call-read",
                    "name":"Read",
                    "input":{"path":"CLAUDE.md"}
                }]
            }),
            json!({
                "role":"user",
                "content":[{
                    "type":"tool_result",
                    "tool_use_id":"call-a",
                    "content":[{"type":"text","text":"teammate_spawned agentId: worker-b"}]
                }]
            }),
            json!({
                "role":"user",
                "content":[{
                    "type":"tool_result",
                    "tool_use_id":"call-c",
                    "content":[{"type":"text","text":"working in the background\nagent_id: worker-c"}]
                }]
            }),
        ]);
        assert!(
            records.iter().any(|record| record.recipient == "worker-b"),
            "{records:?}"
        );
        assert!(
            records.iter().any(|record| record.recipient == "worker-c"),
            "{records:?}"
        );
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

    #[test]
    fn find_reusable_launch_with_empty_scope_returns_none() {
        let launches = vec![LaunchRecord {
            key: "key-1".to_owned(),
            recipient: "worker-1".to_owned(),
            scope: String::new(),
            model: Some("model-1".to_owned()),
            status: "active".to_owned(),
        }];
        let args = json!({"prompt": "", "claudex_model": "model-1"});
        assert!(find_reusable_launch(&launches, &args).is_none());
    }

    #[test]
    fn find_reusable_launch_requires_nonempty_recipient() {
        let launches = vec![LaunchRecord {
            key: "key-1".to_owned(),
            recipient: String::new(),
            scope: "Test scope".to_owned(),
            model: Some("model-1".to_owned()),
            status: "active".to_owned(),
        }];
        let args = json!({"prompt": "Test scope", "claudex_model": "model-1"});
        assert!(find_reusable_launch(&launches, &args).is_none());
    }

    #[test]
    fn already_has_resume_with_nonempty_value() {
        assert!(already_has_resume(&json!({"resume": "worker-a"})));
        assert!(already_has_resume(&json!({"resume": "some-agent"})));
    }

    #[test]
    fn apply_transcript_empty_transcript() {
        let mut launches = vec![];
        apply_transcript(&mut launches, &[]);
        assert!(launches.is_empty());
    }

    #[test]
    fn apply_transcript_with_status_update() {
        let mut launches = vec![LaunchRecord {
            key: "tool-a".to_owned(),
            recipient: "worker-a".to_owned(),
            scope: "Test scope".to_owned(),
            model: Some("model-1".to_owned()),
            status: "active".to_owned(),
        }];
        let messages = vec![json!({
            "role": "user",
            "content": "<task-id>worker-a</task-id><status>completed</status>"
        })];
        apply_transcript(&mut launches, &messages);
        assert_eq!(launches[0].status, "completed");
    }

    #[test]
    fn latest_user_text_empty_messages() {
        assert!(latest_user_text(&[]).is_empty());
    }

    #[test]
    fn latest_user_text_with_text_content() {
        let messages = vec![json!({
            "role": "user",
            "content": [{"type": "text", "text": "user message"}]
        })];
        assert_eq!(latest_user_text(&messages), "user message");
    }

    #[test]
    fn latest_user_text_skips_non_user_roles() {
        let messages = vec![
            json!({"role": "assistant", "content": [{"type": "text", "text": "assistant message"}]}),
            json!({"role": "user", "content": [{"type": "text", "text": "user message"}]}),
        ];
        assert_eq!(latest_user_text(&messages), "user message");
    }

    #[test]
    fn scope_similarity_zero_when_no_overlap() {
        let similarity = scope_similarity("audit rust adapter", "review css layout");
        assert_eq!(similarity, 0);
    }

    #[test]
    fn scope_similarity_nonzero_with_matches() {
        let similarity = scope_similarity("audit rust tests", "continue rust work");
        assert!(similarity > 0);
    }

    #[test]
    fn scope_similarity_filters_short_words() {
        let similarity = scope_similarity("a b c test adapter", "x y z test");
        assert!(similarity == 1);
    }

    #[test]
    fn rewrite_launch_input_with_already_has_resume() {
        let registry = SubagentReuseRegistry::default();
        let mut arguments = json!({
            "prompt": "Test",
            "claudex_model": "model-1",
            "resume": "existing-worker"
        });
        assert!(
            registry
                .rewrite_launch_input("session-a", &mut arguments)
                .is_none()
        );
    }

    #[test]
    fn rewrite_launch_input_empty_session_id() {
        let registry = SubagentReuseRegistry::default();
        let mut arguments = json!({"prompt": "Test", "claudex_model": "model-1"});
        assert!(registry.rewrite_launch_input("", &mut arguments).is_none());
    }

    #[test]
    fn find_reusable_launch_prioritizes_active_over_completed() {
        let launches = vec![
            LaunchRecord {
                key: "key-1".to_owned(),
                recipient: "worker-completed".to_owned(),
                scope: "Test scope".to_owned(),
                model: Some("model-1".to_owned()),
                status: "completed".to_owned(),
            },
            LaunchRecord {
                key: "key-2".to_owned(),
                recipient: "worker-active".to_owned(),
                scope: "Test scope".to_owned(),
                model: Some("model-1".to_owned()),
                status: "active".to_owned(),
            },
        ];
        let args = json!({"prompt": "Test scope", "claudex_model": "model-1"});
        let result = find_reusable_launch(&launches, &args);
        assert_eq!(result.map(|r| r.recipient.as_str()), Some("worker-active"));
    }

    #[test]
    fn apply_transcript_merge_same_recipient() {
        let mut launches = vec![LaunchRecord {
            key: "key-1".to_owned(),
            recipient: "worker-a".to_owned(),
            scope: String::new(),
            model: None,
            status: "active".to_owned(),
        }];
        let messages = vec![
            json!({
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "tool-a",
                    "name": "Agent",
                    "input": {"prompt": "New scope"}
                }]
            }),
            json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "tool-a",
                    "content": [{"type": "text", "text": "Async agent launched successfully.\nagentId: worker-a"}]
                }]
            }),
        ];
        apply_transcript(&mut launches, &messages);
        assert_eq!(launches.len(), 1);
        assert_eq!(launches[0].scope, "New scope");
    }

    #[test]
    fn queued_message_recipient_extraction() {
        let mut launches = vec![LaunchRecord {
            key: "key-a".to_owned(),
            recipient: "worker-a".to_owned(),
            scope: "Test".to_owned(),
            model: Some("model".to_owned()),
            status: "active".to_owned(),
        }];
        let messages = vec![json!({
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": "send-a", "content": "Agent \"worker-a\" had no active task; resumed from transcript in the background."}]
        })];
        apply_transcript(&mut launches, &messages);
        assert_eq!(launches[0].status, "message_queued");
    }

    #[test]
    fn scope_is_occupied_empty_scope_key() {
        let launches = vec![LaunchRecord {
            key: "key-1".to_owned(),
            recipient: "worker-a".to_owned(),
            scope: "Test".to_owned(),
            model: None,
            status: "active".to_owned(),
        }];
        assert!(!scope_is_occupied(&launches, ""));
    }

    #[test]
    fn scope_is_occupied_terminal_status_ignored() {
        let launches = vec![LaunchRecord {
            key: "key-1".to_owned(),
            recipient: "worker-a".to_owned(),
            scope: "Test scope".to_owned(),
            model: None,
            status: "completed".to_owned(),
        }];
        assert!(!scope_is_occupied(&launches, "test scope"));
    }

    #[test]
    fn note_inflight_launch_empty_scope() {
        let registry = SubagentReuseRegistry::default();
        let arguments = json!({"prompt": "", "claudex_model": "model"});
        registry.note_inflight_launch("session-a", &arguments, "tool-a");
        assert!(
            registry
                .state_for("session-a")
                .unwrap_or_default()
                .is_empty()
        );
    }

    #[test]
    fn find_reusable_launch_no_exact_match_returns_none() {
        let launches = vec![LaunchRecord {
            key: "key-1".to_owned(),
            recipient: "worker-a".to_owned(),
            scope: "Audit rust tests".to_owned(),
            model: Some("model-1".to_owned()),
            status: "active".to_owned(),
        }];
        let args = json!({"prompt": "Review CSS", "claudex_model": "model-1"});
        assert!(find_reusable_launch(&launches, &args).is_none());
    }

    #[test]
    fn latest_user_text_prefers_latest_user_message() {
        let messages = vec![
            json!({"role": "user", "content": [{"type": "text", "text": "first"}]}),
            json!({"role": "assistant", "content": [{"type": "text", "text": "response"}]}),
            json!({"role": "user", "content": [{"type": "text", "text": "second"}]}),
        ];
        assert_eq!(latest_user_text(&messages), "second");
    }

    #[test]
    fn scope_similarity_case_insensitive() {
        let sim1 = scope_similarity("Audit RUST Adapter", "audit rust tests");
        let sim2 = scope_similarity("audit rust adapter", "audit rust tests");
        assert_eq!(sim1, sim2);
    }

    #[test]
    fn apply_transcript_empty_content() {
        let mut launches = vec![];
        let messages = vec![json!({"role": "user", "content": []})];
        apply_transcript(&mut launches, &messages);
        assert!(launches.is_empty());
    }

    #[test]
    fn find_reusable_launch_with_model_none() {
        let launches = vec![LaunchRecord {
            key: "key-1".to_owned(),
            recipient: "worker-a".to_owned(),
            scope: "Test scope".to_owned(),
            model: None,
            status: "active".to_owned(),
        }];
        let args = json!({"prompt": "Test scope"});
        let result = find_reusable_launch(&launches, &args);
        assert!(result.is_some());
    }

    #[test]
    fn scope_similarity_minimum_word_length_filter() {
        let similarity = scope_similarity("ab cd audit", "audit test");
        assert!(similarity == 1);
    }

    #[test]
    fn merge_launches_keeps_status_when_observed_status_is_blank() {
        let mut launches = vec![LaunchRecord {
            key: "tool-a".to_owned(),
            recipient: "worker-a".to_owned(),
            scope: "Audit rust".to_owned(),
            model: Some("gpt-test".to_owned()),
            status: "active".to_owned(),
        }];
        super::records::merge_launches(
            &mut launches,
            std::iter::once(&LaunchRecord {
                key: "tool-a".to_owned(),
                recipient: "worker-a".to_owned(),
                scope: "Audit rust".to_owned(),
                model: None,
                status: String::new(),
            }),
        );
        assert_eq!(launches[0].status, "active");
        assert_eq!(launches[0].model.as_deref(), Some("gpt-test"));
    }

    #[test]
    fn apply_transcript_skips_messages_without_content_and_unknown_status_ids() {
        let mut launches = vec![LaunchRecord {
            key: "tool-a".to_owned(),
            recipient: "worker-a".to_owned(),
            scope: "Audit rust".to_owned(),
            model: None,
            status: "active".to_owned(),
        }];
        apply_transcript(
            &mut launches,
            &[
                json!({"role":"user"}),
                json!({"role":"assistant","content":[{"type":"tool_use","name":"Agent"}]}),
                json!({"role":"assistant","content":[{"type":"tool_use","id":"tool-b"}]}),
                json!({
                    "role":"user",
                    "content":"<task-id>missing-task</task-id><status>failed</status>"
                }),
                json!({
                    "role":"user",
                    "content":"Agent \"ghost-worker\" had no active task"
                }),
            ],
        );
        assert_eq!(launches[0].status, "active");
        assert_eq!(launches.len(), 1);
    }

    #[test]
    fn apply_transcript_marks_status_by_launch_key() {
        let mut launches = vec![LaunchRecord {
            key: "tool-a".to_owned(),
            recipient: "worker-a".to_owned(),
            scope: "Audit rust".to_owned(),
            model: None,
            status: "active".to_owned(),
        }];
        apply_transcript(
            &mut launches,
            &[json!({
                "role":"user",
                "content":"<task-id>tool-a</task-id><status>timeout</status>"
            })],
        );
        assert_eq!(launches[0].status, "timeout");
    }

    #[test]
    fn live_agent_task_ids_dedupes_matching_key_and_recipient() {
        let messages = vec![json!({
            "role":"user",
            "content":[{
                "type":"tool_result",
                "tool_use_id":"a4496564387a2561f",
                "content":[{"type":"text","text":"Async agent launched successfully.\nagentId: a4496564387a2561f"}]
            }]
        })];
        assert_eq!(
            live_agent_task_ids(&messages),
            vec!["a4496564387a2561f".to_owned()]
        );
    }
}
