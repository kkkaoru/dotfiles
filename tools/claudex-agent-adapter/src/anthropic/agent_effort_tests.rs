#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Instant;

    use serde_json::{Value, json};

    use super::{
        AgentEffort, AgentEffortIntents, AgentEffortRecord, prepare_arguments,
        prepare_arguments_for_user, tool_schema, validate_routed_agent_arguments,
    };
    use crate::anthropic::MessagesRequest;

    fn request(user_id: &str, prompt: &str, subagent: bool) -> MessagesRequest {
        let marker = if subagent { "cc_is_subagent=true" } else { "" };
        MessagesRequest {
            model: "resolved-model".to_owned(),
            system: json!([{"type":"text","text":marker}]),
            messages: vec![json!({
                "role":"user", "content":[{"type":"text","text":prompt}]
            })],
            tools: Vec::new(),
            stream: false,
            output_config: json!({"effort":"low"}),
            metadata: json!({"user_id":user_id}),
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        }
    }

    impl AgentEffortIntents {
        fn record(
            &self,
            client_user_id: Option<&str>,
            tool_name: &str,
            tool_use_id: String,
            parent_model: &str,
            arguments: &serde_json::Value,
        ) {
            self.record_from_user_messages(
                AgentEffortRecord {
                    client_user_id,
                    tool_name,
                    tool_use_id,
                    parent_model,
                    arguments,
                    user_messages: &[],
                    system: &json!(null),
                },
                None,
            );
        }
    }

    fn explicit(effort: AgentEffort) -> String {
        match effort {
            AgentEffort::Explicit(value) => value,
            AgentEffort::Unmatched | AgentEffort::ConfiguredDefault => {
                panic!("expected explicit Agent effort")
            }
        }
    }

    #[test]
    fn correlates_explicit_effort_by_client_session_and_prompt() {
        let intents = AgentEffortIntents::default();
        intents.record(
            Some("session-a"),
            "Agent",
            "tool-a".to_owned(),
            "main-model",
            &json!({"prompt":"task-a","effort":"high"}),
        );
        assert!(matches!(
            intents.take(&request("session-a", "task-a", false)).effort,
            AgentEffort::Unmatched
        ));
        assert_eq!(
            explicit(intents.take(&request("session-a", "task-a", true)).effort),
            "high"
        );
    }

    #[test]
    fn correlates_parallel_and_repeated_prompts_without_crossing_sessions() {
        let intents = AgentEffortIntents::default();
        intents.record(
            Some("session-a"),
            "Agent",
            "tool-a1".to_owned(),
            "main-model",
            &json!({"prompt":"same","effort":"high"}),
        );
        intents.record(
            Some("session-a"),
            "Agent",
            "tool-a2".to_owned(),
            "main-model",
            &json!({"prompt":"same","effort":"low"}),
        );
        intents.record(
            Some("session-b"),
            "Agent",
            "tool-b".to_owned(),
            "main-model",
            &json!({"prompt":"same","effort":"medium"}),
        );
        assert_eq!(
            explicit(intents.take(&request("session-b", "same", true)).effort),
            "medium"
        );
        assert_eq!(
            explicit(intents.take(&request("session-a", "same", true)).effort),
            "high"
        );
        assert_eq!(
            explicit(intents.take(&request("session-a", "same", true)).effort),
            "low"
        );
    }

    #[test]
    fn unique_markers_correlate_reversed_identical_prompt_launches() {
        let intents = AgentEffortIntents::default();
        let (first, _) = prepare_arguments(
            "Agent",
            "tool-first",
            &json!({"prompt":"same","effort":"high"}),
        );
        let (second, _) = prepare_arguments(
            "Agent",
            "tool-second",
            &json!({"prompt":"same","effort":"low"}),
        );
        intents.record(
            Some("outer-session"),
            "Agent",
            "tool-first".to_owned(),
            "main-model",
            first.as_ref().expect("first intent"),
        );
        intents.record(
            Some("outer-session"),
            "Agent",
            "tool-second".to_owned(),
            "main-model",
            second.as_ref().expect("second intent"),
        );
        let first = first.expect("first intent");
        let second = second.expect("second intent");
        let second_prompt = second["prompt"].as_str().expect("second prompt");
        let first_prompt = first["prompt"].as_str().expect("first prompt");
        let wrapped_second = format!("<teammate-message>{second_prompt}</teammate-message>");
        assert_eq!(
            explicit(
                intents
                    .take(&request_without_user_id(&wrapped_second))
                    .effort
            ),
            "low"
        );
        assert_eq!(
            explicit(intents.take(&request_without_user_id(first_prompt)).effort),
            "high"
        );
        assert_eq!(
            explicit(intents.take(&request_without_user_id(first_prompt)).effort),
            "high"
        );
        intents.remove_tool_results(["tool-first"].into_iter());
        assert_eq!(
            explicit(intents.take(&request_without_user_id(first_prompt)).effort),
            "high"
        );
    }

    #[test]
    fn an_agent_without_explicit_effort_uses_configured_default() {
        let intents = AgentEffortIntents::default();
        intents.record(
            Some("session"),
            "Agent",
            "tool".to_owned(),
            "main-model",
            &json!({"prompt":"task"}),
        );
        assert!(matches!(
            intents.take(&request("session", "task", true)).effort,
            AgentEffort::ConfiguredDefault
        ));
    }

    #[test]
    fn preserves_claude_code_foreground_and_background_launch_modes() {
        let intents = AgentEffortIntents::default();
        intents.record_from_user_messages(
            AgentEffortRecord {
                client_user_id: Some("session"),
                tool_name: "Agent",
                tool_use_id: "foreground".to_owned(),
                parent_model: "main-model",
                arguments: &json!({"prompt":"foreground","run_in_background":false}),
                user_messages: &[
                    json!({"role":"user","content":"同期で結果を待ってから次へ進めて"}),
                ],
                system: &json!(null),
            },
            None,
        );
        intents.record_from_user_messages(
            AgentEffortRecord {
                client_user_id: Some("session"),
                tool_name: "Agent",
                tool_use_id: "background".to_owned(),
                parent_model: "main-model",
                arguments: &json!({"prompt":"background","run_in_background":false}),
                user_messages: &[json!({
                    "role":"user",
                    "content":"Investigate how sync-realtime-data chooses the writable Neon connection."
                })],
                system: &json!(null),
            },
            None,
        );

        let foreground = intents.take(&request("session", "foreground", true));
        let background = intents.take(&request("session", "background", true));
        assert!(!foreground.run_in_background);
        assert!(background.run_in_background);
    }

    #[test]
    fn forces_explore_background_unless_user_requires_sync() {
        let (_, queued) = prepare_arguments_for_user(
            "Agent",
            "tool-explore",
            &json!({
                "prompt":"Assess production configuration paths",
                "subagent_type":"Explore",
                "run_in_background":false
            }),
            &[json!({
                "role":"user",
                "content":"Investigate how sync-realtime-data chooses the writable Neon connection."
            })],
            &json!(null),
        );
        assert_eq!(queued["run_in_background"], true);

        let (_, hurry) = prepare_arguments_for_user(
            "Agent",
            "tool-hurry",
            &json!({
                "prompt":"Assess production configuration paths",
                "subagent_type":"Explore",
                "run_in_background":false
            }),
            &[json!({"role":"user","content":"さっさとやれ"})],
            &json!(null),
        );
        assert_eq!(hurry["run_in_background"], true);

        let (_, sync) = prepare_arguments_for_user(
            "Agent",
            "tool-sync",
            &json!({
                "prompt":"Assess production configuration paths",
                "subagent_type":"Explore",
                "run_in_background":false
            }),
            &[json!({"role":"user","content":"同期で結果を待ってから次へ進めて"})],
            &json!(null),
        );
        assert_eq!(sync["run_in_background"], false);
    }

    #[test]
    fn correlation_marker_identifies_subagent_without_billing_header() {
        let intents = AgentEffortIntents::default();
        let (internal, _) = prepare_arguments(
            "Agent",
            "tool-background",
            &json!({"prompt":"background task"}),
        );
        let internal = internal.expect("agent intent");
        intents.record(
            None,
            "Agent",
            "tool-background".to_owned(),
            "main-model",
            &internal,
        );
        intents.remove_tool_results(["tool-background"].into_iter());

        let intent = intents.take(&request(
            "session",
            internal["prompt"].as_str().expect("correlated prompt"),
            false,
        ));
        assert!(intent.is_subagent);
        assert!(intent.model_override.is_none());
        assert!(matches!(intent.effort, AgentEffort::ConfiguredDefault));
    }

    #[test]
    fn outer_follow_up_does_not_consume_a_prior_agent_marker_without_tool_result() {
        let intents = AgentEffortIntents::default();
        let (internal, _) = prepare_arguments(
            "Agent",
            "tool-background",
            &json!({
                "prompt":"background task",
                "claudex_model":"worker-model",
                "claudex_effort":"high"
            }),
        );
        let internal = internal.expect("agent intent");
        intents.record(
            Some("outer-session"),
            "Agent",
            "tool-background".to_owned(),
            "main-model",
            &internal,
        );
        let prompt = internal["prompt"].as_str().expect("correlated prompt");
        let request = MessagesRequest {
            model: "main-model".to_owned(),
            system: json!("main session"),
            messages: vec![
                json!({"role":"user","content":"launch the worker"}),
                json!({
                    "role":"assistant",
                    "content":[{"type":"tool_use","name":"Agent","input":{"prompt":prompt}}]
                }),
                json!({"role":"user","content":"continue the main answer"}),
            ],
            tools: Vec::new(),
            stream: false,
            output_config: Value::Null,
            metadata: json!({"user_id":"outer-session"}),
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        };

        let intent = intents.take(&request);

        assert!(!intent.is_subagent);
        assert!(!intent.matched);
        assert!(intent.model_override.is_none());
    }

    #[test]
    fn plain_launch_id_matches_each_concurrent_background_agent() {
        let intents = AgentEffortIntents::default();
        for (tool_use_id, effort) in [("tool-first", "low"), ("tool-second", "xhigh")] {
            let (arguments, _) = prepare_arguments(
                "Agent",
                tool_use_id,
                &json!({"prompt":"parallel work", "claudex_effort":effort}),
            );
            intents.record(
                Some("session"),
                "Agent",
                tool_use_id.to_owned(),
                "main-model",
                &arguments.expect("correlated Agent intent"),
            );
        }

        let request = request(
            "session",
            "work payload\nclaudex_launch_id: tool-second",
            true,
        );
        assert_eq!(explicit(intents.take(&request).effort), "xhigh");
    }

    #[test]
    fn restores_correlated_launch_intents_after_adapter_handover() {
        let root = tempfile::tempdir().expect("intent journal directory");
        let path = root.path().join("agent-intents.json");
        let (arguments, _) = prepare_arguments(
            "Agent",
            "tool-handover",
            &json!({
                "prompt":"resume this worker", "claudex_model":"grok-4.5",
                "claudex_effort":"xhigh"
            }),
        );
        let arguments = arguments.expect("correlated Agent intent");
        let user_messages = [json!({
            "role":"user", "content":"Use grok-4.5 for this SubAgent"
        })];
        AgentEffortIntents::with_store(path.clone()).record_from_user_messages(
            AgentEffortRecord {
                client_user_id: Some("outer-session"),
                tool_name: "Agent",
                tool_use_id: "tool-handover".to_owned(),
                parent_model: "main-model",
                arguments: &arguments,
                user_messages: &user_messages,
                system: &json!(null),
            },
            None,
        );

        let restored = AgentEffortIntents::with_store(path);
        let intent = restored.take(&request_without_user_id(
            arguments["prompt"].as_str().expect("correlated prompt"),
        ));
        assert!(intent.matched);
        assert_eq!(intent.model_override.as_deref(), Some("grok-4.5"));
        assert_eq!(explicit(intent.effort), "xhigh");
    }

    #[test]
    fn recovers_a_unique_correlated_intent_after_context_compaction() {
        let intents = AgentEffortIntents::default();
        let (arguments, _) = prepare_arguments(
            "Agent",
            "tool-compacted",
            &json!({"prompt":"large worker", "claudex_effort":"high"}),
        );
        intents.record(
            Some("outer-session"),
            "Agent",
            "tool-compacted".to_owned(),
            "main-model",
            &arguments.expect("correlated Agent arguments"),
        );
        let intent = intents.take(&request("outer-session", "retained suffix only", true));
        assert!(intent.matched);
        assert_eq!(explicit(intent.effort), "high");
    }

    #[test]
    fn does_not_guess_between_multiple_compacted_correlations() {
        let intents = AgentEffortIntents::default();
        for id in ["tool-compacted-a", "tool-compacted-b"] {
            let (arguments, _) =
                prepare_arguments("Agent", id, &json!({"prompt":id, "claudex_effort":"high"}));
            intents.record(
                Some("outer-session"),
                "Agent",
                id.to_owned(),
                "main-model",
                &arguments.expect("correlated Agent arguments"),
            );
        }
        assert!(matches!(
            intents
                .take(&request("outer-session", "retained suffix only", true))
                .effort,
            AgentEffort::Unmatched
        ));
    }

    #[test]
    fn correlated_intent_survives_time_and_refreshes_lru() {
        assert_eq!(super::INTENT_TTL, std::time::Duration::from_secs(10 * 60));
        let intents = AgentEffortIntents::default();
        let (internal, _) = prepare_arguments(
            "Agent",
            "tool-reused",
            &json!({"prompt":"initial task","claudex_effort":"high"}),
        );
        let internal = internal.expect("correlated agent intent");
        intents.record(
            None,
            "Agent",
            "tool-reused".to_owned(),
            "provider-model",
            &internal,
        );
        let (second, _) = prepare_arguments(
            "Agent",
            "tool-second",
            &json!({"prompt":"second task","claudex_effort":"low"}),
        );
        intents.record(
            None,
            "Agent",
            "tool-second".to_owned(),
            "second-model",
            second.as_ref().expect("second correlated intent"),
        );
        intents.pending.lock().unwrap()[0].created_at =
            Instant::now() - std::time::Duration::from_secs(121 * 60);

        assert!(intents.pending.lock().unwrap()[0].prompt.is_empty());

        let reused = intents.take(&request_without_user_id(
            internal["prompt"].as_str().expect("correlated prompt"),
        ));

        assert!(reused.model_override.is_none());
        assert_eq!(explicit(reused.effort), "high");
        let pending = intents.pending.lock().unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending.front().unwrap().tool_use_id, "tool-second");
        assert_eq!(pending.back().unwrap().tool_use_id, "tool-reused");
    }

    #[test]
    fn preserves_agent_schema_and_strips_adapter_only_arguments() {
        for tool_name in ["Agent", "Task"] {
            let received = json!({
                "type":"object",
                "properties":{"prompt":{"type":"string"}},
                "required":["prompt"],
                "additionalProperties":false
            });
            assert_eq!(tool_schema(tool_name, received.clone()), received);
            let tool_use_id = format!("tool-mid-{tool_name}");
            let (internal, public) = prepare_arguments(
                tool_name,
                &tool_use_id,
                &json!({"prompt":"task","claudex_effort":"mid"}),
            );
            let internal = internal.expect("agent intent");
            assert_eq!(internal["claudex_effort"], "mid");
            assert!(public.get("claudex_effort").is_none());

            let intents = AgentEffortIntents::default();
            intents.record(None, tool_name, tool_use_id, "main-model", &internal);
            assert_eq!(
                explicit(
                    intents
                        .take(&request_without_user_id(
                            internal["prompt"].as_str().expect("correlated prompt")
                        ))
                        .effort
                ),
                "medium"
            );
        }
    }

    #[test]
    fn recovers_subscription_routing_headers_from_task_prompts() {
        let arguments = json!({
            "prompt":"claudex_model: gpt-5.6-sol\nclaudex_effort: high\n\nDo the task",
            "model":"gpt-5.6-sol"
        });
        let (internal, public) = prepare_arguments_for_user(
            "Task",
            "tool-subscription",
            &arguments,
            &[json!({"role":"user","content":"selected model gpt-5.6-sol"})],
            &json!(null),
        );
        let internal = internal.expect("Task routing intent");
        assert_eq!(internal["claudex_model"], "gpt-5.6-sol");
        assert_eq!(internal["claudex_effort"], "high");
        assert!(public.get("model").is_none());
        assert!(public.get("claudex_model").is_none());
        assert!(public.get("claudex_effort").is_none());
    }

    #[test]
    fn strips_adapter_routing_fields_from_non_agent_tool_arguments() {
        let (_, public) = prepare_arguments_for_user(
            "Monitor",
            "tool-monitor",
            &json!({
                "path":"/tmp/status",
                "claudex_model":"gpt-5.6-luna",
                "claudex_implicit_model":"gpt-5.6-luna",
                "claudex_effort":"max"
            }),
            &[],
            &json!(null),
        );

        assert_eq!(public, json!({"path":"/tmp/status"}));
    }

    #[test]
    fn hydrates_explicit_claudex_model_without_vendor_prefix_inference() {
        let (internal, public) = prepare_arguments(
            "Task",
            "tool-provider",
            &json!({
                "prompt":"investigate",
                "model":"sonnet",
                "claudex_model":"vendor-next",
                "claudex_effort":"high"
            }),
        );
        let internal = internal.expect("Task routing intent");
        assert_eq!(internal["claudex_model"], "vendor-next");
        assert_eq!(internal["claudex_effort"], "high");
        // Claude Code short aliases in `model` must not become claudex_model.
        assert_ne!(internal.get("claudex_model"), Some(&json!("sonnet")));
        assert!(public.get("model").is_none());
        assert!(public.get("claudex_model").is_none());
    }

    #[test]
    fn hydrates_and_authorizes_a_configured_worker_without_proxy_fields() {
        let root = tempfile::tempdir().expect("provider config directory");
        let path = root.path().join("providers.json");
        std::fs::write(
            &path,
            r#"{"version":1,"mainProviders":["grok"],"providers":[{"id":"grok","agent":"claudex-grok","defaultModel":"grok-4.5","effort":"high","backend":"grok-acp"}],"fallback":{"agent":"claudex-sonnet","model":"claude-sonnet-5","effort":"high"}}"#,
        )
        .expect("write provider config");
        let catalog = crate::provider_config::load(&path)
            .expect("load provider config")
            .model_catalog;
        let mut arguments = json!({"subagent_type":"claudex-grok","prompt":"research"});

        super::super::agent_routing::hydrate_routing_fields_from_context(
            &mut arguments,
            &[],
            &json!(null),
            &catalog,
        );

        assert_eq!(arguments["claudex_model"], "grok-4.5");
        assert_eq!(arguments["claudex_effort"], "high");
        super::validate_routed_agent_arguments_with_catalog(
            "Agent",
            &arguments,
            &[],
            &json!(null),
            &catalog,
        )
        .expect("configured worker must be authorized");
        let (intent_arguments, _) = prepare_arguments("Agent", "tool-configured", &arguments);
        let intent_arguments = intent_arguments.expect("configured Agent intent");
        let intents = AgentEffortIntents::default();
        intents.record_from_user_messages(
            AgentEffortRecord {
                client_user_id: None,
                tool_name: "Agent",
                tool_use_id: "tool-configured".to_owned(),
                parent_model: "parent-model",
                arguments: &intent_arguments,
                user_messages: &[],
                system: &json!(null),
            },
            Some(&catalog),
        );
        assert_eq!(
            intents
                .take(&request_without_user_id(
                    intent_arguments["prompt"]
                        .as_str()
                        .expect("correlated prompt"),
                ))
                .model_override
                .as_deref(),
            Some("grok-4.5")
        );
    }

    #[test]
    fn live_routing_does_not_reauthorize_capacity_excluded_catalog_workers() {
        let mut catalog = crate::provider_config::ModelCatalog::default();
        catalog
            .set_worker_routes(vec![
                crate::provider_config::WorkerRoute::new(
                    "claudex-ollama-glm-5-2".to_owned(),
                    "glm-5.2:cloud".to_owned(),
                    "max".to_owned(),
                ),
                crate::provider_config::WorkerRoute::new(
                    "claudex-cline-deepseek-flash".to_owned(),
                    "cline-pass/deepseek-v4-flash".to_owned(),
                    "xhigh".to_owned(),
                ),
            ])
            .expect("valid worker routes");
        let messages = [json!({
            "role":"user",
            "content":"Claudex routing for this turn: {\"providers\":{},\"selected_workers\":[{\"agent\":\"claudex-cline-deepseek-flash\",\"model\":\"cline-pass/deepseek-v4-flash\",\"effort\":\"xhigh\"}]} mandatory policy"
        })];
        let mut generic = json!({
            "subagent_type":"general-purpose",
            "prompt":"should stay on the automatic selected pool"
        });

        super::super::agent_routing::hydrate_routing_fields_from_context(
            &mut generic,
            &messages,
            &json!(null),
            &catalog,
        );
        assert_eq!(generic["claudex_model"], "cline-pass/deepseek-v4-flash");
        let error = super::validate_routed_agent_arguments_with_catalog(
            "Agent",
            &json!({
                "subagent_type":"general-purpose",
                "claudex_model":"glm-5.2:cloud",
                "claudex_effort":"max",
                "prompt":"should not inherit a capacity-excluded catalog worker"
            }),
            &messages,
            &json!(null),
            &catalog,
        )
        .expect_err("generic Agent must not use a catalog worker outside selected_workers");
        assert!(
            error.to_string().contains("does not match the exact route"),
            "{error}"
        );
    }

    #[test]
    fn named_catalog_worker_survives_stale_selected_workers_after_cline_exhaustion() {
        let mut catalog = crate::provider_config::ModelCatalog::default();
        catalog
            .set_worker_routes(vec![
                crate::provider_config::WorkerRoute::new(
                    "claudex-cline-deepseek-flash".to_owned(),
                    "cline-pass/deepseek-v4-flash".to_owned(),
                    "xhigh".to_owned(),
                ),
                crate::provider_config::WorkerRoute::new(
                    "claudex-ollama-glm-5-2".to_owned(),
                    "glm-5.2:cloud".to_owned(),
                    "max".to_owned(),
                ),
                crate::provider_config::WorkerRoute::new(
                    "claudex-qwen".to_owned(),
                    "qwen3.8-max-preview".to_owned(),
                    "high".to_owned(),
                ),
            ])
            .expect("valid worker routes");
        let messages = [json!({
            "role":"user",
            "content":"continue\nClaudex routing for this turn: {\"providers\":{},\"selected_agents\":[\"claudex-cline-deepseek-flash\",\"claudex-ollama-glm-5-2\"],\"selected_workers\":[{\"agent\":\"claudex-cline-deepseek-flash\",\"model\":\"cline-pass/deepseek-v4-flash\",\"effort\":\"xhigh\"},{\"agent\":\"claudex-ollama-glm-5-2\",\"model\":\"glm-5.2:cloud\",\"effort\":\"max\"}]} mandatory policy"
        })];

        let mut unnamed = json!({
            "subagent_type":"claudex-qwen",
            "prompt":"reroute after Cline Credits returned empty"
        });
        super::super::agent_routing::hydrate_routing_fields_from_context(
            &mut unnamed,
            &messages,
            &json!(null),
            &catalog,
        );
        assert_eq!(unnamed["claudex_model"], "qwen3.8-max-preview");
        assert_eq!(unnamed["claudex_effort"], "high");
        super::validate_routed_agent_arguments_with_catalog(
            "Agent",
            &unnamed,
            &messages,
            &json!(null),
            &catalog,
        )
        .expect("named Qwen worker must launch after Cline exhaustion");

        let explicit = json!({
            "subagent_type":"claudex-qwen",
            "claudex_model":"qwen3.8-max-preview",
            "claudex_effort":"high",
            "prompt":"reroute after Cline Credits returned empty"
        });
        super::validate_routed_agent_arguments_with_catalog(
            "Agent",
            &explicit,
            &messages,
            &json!(null),
            &catalog,
        )
        .expect("explicit Qwen model must match its catalog route");

        let error = super::validate_routed_agent_arguments_with_catalog(
            "Agent",
            &json!({
                "subagent_type":"claudex-qwen",
                "claudex_model":"not-a-configured-worker",
                "prompt":"unknown sibling must still be rejected"
            }),
            &messages,
            &json!(null),
            &catalog,
        )
        .expect_err("unknown Qwen model must keep the old exact-route rejection");
        assert!(
            error.to_string().contains("does not match the exact route"),
            "{error}"
        );
    }

    #[test]
    fn rewritten_qwen_launch_authorizes_after_stale_cline_snapshot() {
        let mut catalog = crate::provider_config::ModelCatalog::default();
        catalog
            .set_worker_routes(vec![
                crate::provider_config::WorkerRoute::new(
                    "claudex-cline-deepseek-flash".to_owned(),
                    "cline-pass/deepseek-v4-flash".to_owned(),
                    "xhigh".to_owned(),
                ),
                crate::provider_config::WorkerRoute::new(
                    "claudex-qwen".to_owned(),
                    "qwen3.8-max-preview".to_owned(),
                    "high".to_owned(),
                ),
            ])
            .expect("valid worker routes");
        let messages = [json!({
            "role":"user",
            "content":"continue\nClaudex routing for this turn: {\"providers\":{},\"selected_workers\":[{\"agent\":\"claudex-cline-deepseek-flash\",\"model\":\"cline-pass/deepseek-v4-flash\",\"effort\":\"xhigh\"}]} mandatory policy"
        })];
        let rewritten = json!({
            "subagent_type":"claudex-qwen",
            "claudex_model":"qwen3.8-max-preview",
            "claudex_effort":"high",
            "prompt":"nested launch after Cline cooldown"
        });
        super::validate_routed_agent_arguments_with_catalog(
            "Agent",
            &rewritten,
            &messages,
            &json!(null),
            &catalog,
        )
        .expect("rewritten Qwen worker must pass exact-route after Cline cooldown");
    }

    #[test]
    fn standard_agent_types_use_claudex_worker_model_and_effort() {
        let mut catalog = crate::provider_config::ModelCatalog::default();
        catalog
            .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
                "claudex-worker".to_owned(),
                "worker-model".to_owned(),
                "max".to_owned(),
            )])
            .expect("valid worker route");

        let mut general = json!({"subagent_type":"general-purpose","prompt":"inspect"});
        super::super::agent_routing::hydrate_routing_fields_from_context(
            &mut general,
            &[],
            &json!(null),
            &catalog,
        );
        assert_eq!(general["claudex_model"], "worker-model");
        assert_eq!(general["claudex_effort"], "max");
        assert!(general.get("claudex_implicit_model").is_none());
        assert!(
            super::super::agent_routing::model_is_authorized_with_catalog(
                &general,
                &[],
                &json!(null),
                &catalog,
                "worker-model",
            )
        );

        let selected_summary = [json!({
            "role":"user",
            "content":"Claudex routing for this turn: {\"providers\":{},\"selected_workers\":[{\"agent\":\"claudex-worker\",\"model\":\"selected-model\",\"effort\":\"high\"}]}"
        })];
        let mut explore = json!({"subagent_type":"Explore","prompt":"inspect"});
        super::super::agent_routing::hydrate_routing_fields_from_context(
            &mut explore,
            &selected_summary,
            &json!(null),
            &catalog,
        );
        assert_eq!(explore["claudex_model"], "selected-model");
        assert_eq!(explore["claudex_effort"], "high");
        assert!(explore.get("claudex_implicit_model").is_none());

        let mut routed = json!({"subagent_type":"claudex-gpt"});
        super::super::agent_routing::hydrate_standard_agent_to_parent(
            &mut routed,
            "claude-sonnet-5",
        );
        assert!(routed.get("claudex_model").is_none());

        let mut native_claude = json!({"subagent_type":"claude","prompt":"inspect"});
        super::super::agent_routing::hydrate_standard_agent_to_parent(
            &mut native_claude,
            "gpt-5.6-luna",
        );
        assert_eq!(native_claude["claudex_model"], "claude-haiku-4-5");
        assert!(
            super::super::agent_routing::model_is_authorized_with_catalog(
                &native_claude,
                &[],
                &json!(null),
                &crate::provider_config::ModelCatalog::default(),
                "claude-haiku-4-5",
            )
        );
    }

    #[test]
    fn named_worker_cannot_mix_another_workers_model_effort_or_implicit_marker() {
        let mut catalog = crate::provider_config::ModelCatalog::default();
        catalog
            .set_worker_routes(vec![
                crate::provider_config::WorkerRoute::new(
                    "worker-a".to_owned(),
                    "model-a".to_owned(),
                    "high".to_owned(),
                ),
                crate::provider_config::WorkerRoute::new(
                    "worker-b".to_owned(),
                    "model-b".to_owned(),
                    "max".to_owned(),
                ),
            ])
            .expect("valid worker routes");
        let messages = [json!({
            "role":"user",
            "content":"Claudex routing for this turn: {\"providers\":{},\"selected_workers\":[{\"agent\":\"worker-a\",\"model\":\"model-a\",\"effort\":\"high\"},{\"agent\":\"worker-b\",\"model\":\"model-b\",\"effort\":\"max\"}]}"
        })];
        let mut mixed = json!({
            "subagent_type":"worker-a",
            "prompt":"inspect",
            "claudex_model":"model-b",
            "claudex_effort":"max",
            "claudex_implicit_model":"model-b"
        });
        super::super::agent_routing::hydrate_routing_fields_from_context(
            &mut mixed,
            &messages,
            &json!(null),
            &catalog,
        );
        assert_eq!(mixed["claudex_model"], "model-a");
        assert_eq!(mixed["claudex_effort"], "high");
        assert!(mixed.get("claudex_implicit_model").is_none());
        super::validate_routed_agent_arguments_with_catalog(
            "Agent",
            &mixed,
            &messages,
            &json!(null),
            &catalog,
        )
        .expect("canonical exact tuple is accepted");
    }

    #[test]
    fn rejects_effort_that_disagrees_with_an_implicit_model_route() {
        let mut catalog = crate::provider_config::ModelCatalog::default();
        catalog
            .set_worker_routes(vec![crate::provider_config::WorkerRoute::new(
                "claudex-worker".to_owned(),
                "worker-model".to_owned(),
                "high".to_owned(),
            )])
            .expect("valid worker route");
        let arguments = json!({
            "subagent_type":"claude",
            "claudex_model":"worker-model",
            "claudex_implicit_model":"worker-model",
            "claudex_effort":"max"
        });

        let error = super::validate_routed_agent_arguments_with_catalog(
            "Agent",
            &arguments,
            &[],
            &json!(null),
            &catalog,
        )
        .expect_err("implicit model effort mismatch must be rejected");
        assert!(error.to_string().contains("does not match `high`"));
    }

    #[test]
    fn generic_agent_uses_the_declared_default_subagent_route() {
        let messages = [json!({
            "role":"user",
            "content":"Claudex routing for this turn: {\"providers\":{},\"default_subagent_route\":{\"agent\":\"worker-b\",\"model\":\"model-b\",\"effort\":\"max\"},\"selected_workers\":[{\"agent\":\"worker-a\",\"model\":\"model-a\",\"effort\":\"high\"},{\"agent\":\"worker-b\",\"model\":\"model-b\",\"effort\":\"max\"}]}"
        })];
        let mut arguments = json!({"subagent_type":"general-purpose","prompt":"inspect"});

        super::super::agent_routing::hydrate_routing_fields_from_context(
            &mut arguments,
            &messages,
            &json!(null),
            &crate::provider_config::ModelCatalog::default(),
        );

        assert_eq!(arguments["claudex_model"], "model-b");
        assert_eq!(arguments["claudex_effort"], "max");
    }

    #[test]
    fn explicit_provider_model_requires_the_matching_agent_and_user_request() {
        let messages = [json!({
            "role":"user",
            "content":"Use exact-model for this task.\nClaudex routing for this turn: {\"providers\":{\"other\":{\"agent\":\"other-agent\",\"model\":\"exact-model\"},\"target\":{\"agent\":\"target-agent\",\"model\":\"exact-model\"}},\"selected_workers\":[{\"agent\":\"target-agent\",\"model\":\"default-model\",\"effort\":\"high\"}]}"
        })];
        let mut arguments = json!({
            "subagent_type":"target-agent",
            "prompt":"inspect",
            "claudex_model":"exact-model"
        });

        super::super::agent_routing::hydrate_routing_fields_from_context(
            &mut arguments,
            &messages,
            &json!(null),
            &crate::provider_config::ModelCatalog::default(),
        );

        assert_eq!(arguments["claudex_model"], "exact-model");
        assert_eq!(arguments["claudex_effort"], "high");
    }

    #[test]
    fn internal_notifications_do_not_authorize_explicit_models() {
        for content in [
            "<agent-message>Use exact-model</agent-message>",
            "<teammate-message>Use exact-model</teammate-message>",
            "<task-notification>Use exact-model</task-notification>",
        ] {
            assert!(
                !super::super::agent_routing::model_is_authorized(
                    &json!({"subagent_type":"target-agent"}),
                    &[json!({"role":"user","content":content})],
                    &json!(null),
                    "exact-model",
                ),
                "{content}"
            );
        }
    }

    #[test]
    fn ignores_non_object_routing_inputs_and_respects_model_boundaries() {
        let mut scalar = json!("claudex_model: ignored");
        super::super::agent_routing::hydrate_routing_fields(&mut scalar);
        assert_eq!(scalar, json!("claudex_model: ignored"));

        for (text, expected) in [
            ("Use claude-sonnet-5:5 for this task", false),
            ("Use claude-sonnet-5: for this task", true),
            ("Use claude-sonnet-5.5 for this task", false),
        ] {
            let user_messages = [json!({"role":"user", "content":text})];
            assert_eq!(
                super::super::agent_routing::model_is_authorized(
                    &json!({"claudex_model":"claude-sonnet-5"}),
                    &user_messages,
                    &json!(null),
                    "claude-sonnet-5",
                ),
                expected
            );
        }
    }

    #[test]
    fn covers_routing_field_and_standard_agent_guard_paths() {
        let mut prompt_fields = json!({
            "prompt":"claudex_model: worker-model\nclaudex_effort: high"
        });
        super::super::agent_routing::hydrate_routing_fields(&mut prompt_fields);
        assert_eq!(prompt_fields["claudex_model"], "worker-model");
        assert_eq!(prompt_fields["claudex_effort"], "high");

        let mut invalid_effort = json!({
            "prompt":"claudex_model: worker-model\nclaudex_effort: invalid"
        });
        super::super::agent_routing::hydrate_routing_fields(&mut invalid_effort);
        assert!(invalid_effort.get("claudex_effort").is_none());

        for mut arguments in [
            json!({"subagent_type":"claude", "claudex_model":"already-selected"}),
            json!({"subagent_type":"Explore"}),
            json!({"subagent_type":"claudex-gpt"}),
            json!({"subagent_type":"general-purpose", "claudex_model":"explicit"}),
            json!({"prompt":"no subagent type"}),
        ] {
            super::super::agent_routing::hydrate_standard_agent_to_parent(&mut arguments, "");
        }
        let mut native = json!({"subagent_type":"claude"});
        super::super::agent_routing::hydrate_standard_agent_to_parent(&mut native, "parent-model");
        assert_eq!(native["claudex_model"], "claude-haiku-4-5");

        let malformed_summary = [json!({
            "role":"user",
            "content":"Claudex routing for this turn: {\"providers\":{},\"selected_workers\":[{\"agent\":\"worker\"}]}"
        })];
        let mut malformed = json!({"subagent_type":"worker"});
        super::super::agent_routing::hydrate_routing_fields_from_context(
            &mut malformed,
            &malformed_summary,
            &json!(null),
            &crate::provider_config::ModelCatalog::default(),
        );
        assert!(malformed.get("claudex_model").is_none());

        let mut selected = json!({"subagent_type":"worker"});
        let summary = [json!({
            "role":"user",
            "content":"Claudex routing for this turn: {\"providers\":{},\"selected_workers\":[{\"agent\":\"worker\",\"model\":\"worker-model\",\"effort\":\"invalid\"}]}"
        })];
        super::super::agent_routing::hydrate_routing_fields_from_context(
            &mut selected,
            &summary,
            &json!(null),
            &crate::provider_config::ModelCatalog::default(),
        );
        assert_eq!(selected["claudex_model"], "worker-model");
        assert!(selected.get("claudex_effort").is_none());

        let mut scalar = Value::String("not an object".to_owned());
        super::super::agent_routing::hydrate_routing_fields_from_context(
            &mut scalar,
            &summary,
            &json!(null),
            &crate::provider_config::ModelCatalog::default(),
        );
        assert_eq!(scalar, Value::String("not an object".to_owned()));
    }

    #[test]
    fn removes_invented_mailbox_names_but_preserves_user_supplied_names() {
        let arguments = json!({
            "prompt":"audit contracts", "name":"wf_contract_audit",
            "subagent_type":"general-purpose"
        });
        let ordinary = [json!({"role":"user","content":"Run a contract audit SubAgent"})];
        let (_, public) = prepare_arguments_for_user(
            "Agent",
            "tool-ordinary",
            &arguments,
            &ordinary,
            &json!(null),
        );
        assert!(public.get("name").is_none());
        let (_, public) = prepare_arguments_for_user(
            "Agent",
            "tool-teammate",
            &json!({"prompt":"audit contracts", "name":"wf_contract_audit"}),
            &[json!({
                "role":"user",
                "content":"<teammate-message>keep mailbox context</teammate-message>"
            })],
            &json!(null),
        );
        assert!(public.get("name").is_none());
        let (_, public) = prepare_arguments_for_user(
            "Agent",
            "tool-coincidental",
            &json!({"prompt":"audit contracts", "name":"audit"}),
            &ordinary,
            &json!(null),
        );
        assert!(public.get("name").is_none());

        let explicit = [json!({
            "role":"user", "content":"Use the named teammate wf_contract_audit"
        })];
        let (_, public) =
            prepare_arguments_for_user("Agent", "tool-named", &arguments, &explicit, &json!(null));
        assert_eq!(public["name"], "wf_contract_audit");

        let stale = [
            json!({"role":"user","content":"Earlier I named wf_contract_audit"}),
            json!({"role":"user","content":"Run another ordinary SubAgent"}),
            json!({"role":"user","content":"<agent-message from=\"wf_contract_audit\">done</agent-message>"}),
        ];
        let (_, public) =
            prepare_arguments_for_user("Agent", "tool-stale", &arguments, &stale, &json!(null));
        assert!(public.get("name").is_none());

        let schema = json!({
            "type":"object", "properties":{"name":{"type":"string","description":"native"}}
        });
        assert_eq!(tool_schema("Agent", schema.clone()), schema);
    }

    #[test]
    fn preserves_fixed_and_arbitrary_explicit_provider_models() {
        let intents = AgentEffortIntents::default();
        let (inherited, public) = prepare_arguments(
            "Agent",
            "tool-inherited",
            &json!({"prompt":"inherit","model":"sonnet"}),
        );
        let inherited = inherited.expect("inherited model intent");
        assert!(public.get("model").is_none());
        intents.record(
            None,
            "Agent",
            "tool-inherited".to_owned(),
            "parent-model",
            &inherited,
        );
        let intent = intents.take(&request_without_user_id(
            inherited["prompt"].as_str().expect("inherited prompt"),
        ));
        assert!(intent.model_override.is_none());

        for model in ["gpt-5.6-sol", "grok-4.5", "claude-opus-4-8"] {
            let tool_id = format!("tool-{model}");
            let (explicit, public) = prepare_arguments(
                "Agent",
                &tool_id,
                &json!({
                    "prompt":model, "model":"sonnet", "claudex_model":model
                }),
            );
            let explicit = explicit.expect("explicit model intent");
            assert!(public.get("model").is_none());
            assert!(public.get("claudex_model").is_none());
            let user_messages = [json!({
                "role":"user", "content":format!("Use {model} for this SubAgent")
            })];
            intents.record_from_user_messages(
                AgentEffortRecord {
                    client_user_id: None,
                    tool_name: "Agent",
                    tool_use_id: tool_id,
                    parent_model: "parent-model",
                    arguments: &explicit,
                    user_messages: &user_messages,
                    system: &json!(null),
                },
                None,
            );
            let intent = intents.take(&request_without_user_id(
                explicit["prompt"].as_str().expect("explicit prompt"),
            ));
            assert_eq!(intent.model_override.as_deref(), Some(model));
        }
    }

    #[test]
    fn ignores_inferred_model_unless_current_user_input_names_exact_id() {
        let intents = AgentEffortIntents::default();
        for (tool_id, user_text, expected) in [
            ("tool-omitted", "Run the commit command", None),
            (
                "tool-prefix-only",
                "Use claude-sonnet-5-newer for this SubAgent",
                None,
            ),
            (
                "tool-dot-suffix",
                "Use claude-sonnet-5.1 for this SubAgent",
                None,
            ),
            (
                "tool-explicit",
                "Use claude-sonnet-5.",
                Some("claude-sonnet-5"),
            ),
        ] {
            let (arguments, _) = prepare_arguments(
                "Agent",
                tool_id,
                &json!({
                    "prompt":"analyze changes",
                    "claudex_model":"claude-sonnet-5"
                }),
            );
            let arguments = arguments.expect("Agent intent");
            let user_messages = [json!({"role":"user", "content":user_text})];
            intents.record_from_user_messages(
                AgentEffortRecord {
                    client_user_id: None,
                    tool_name: "Agent",
                    tool_use_id: tool_id.to_owned(),
                    parent_model: "parent-model",
                    arguments: &arguments,
                    user_messages: &user_messages,
                    system: &json!(null),
                },
                None,
            );
            let intent = intents.take(&request_without_user_id(
                arguments["prompt"].as_str().expect("correlated prompt"),
            ));
            assert_eq!(intent.model_override.as_deref(), expected);
        }
    }

    #[test]
    fn authorizes_exact_selected_worker_model_without_matching_display_agent_type() {
        let intents = AgentEffortIntents::default();
        let routing = r#"Claudex routing for this turn: {"providers":{},"selected_agents":["claudex-gpt-spark"],"selected_workers":[{"agent":"claudex-gpt-spark","model":"gpt-5.3-codex-spark","effort":"high"}]} mandatory policy"#;
        for (tool_id, arguments, expected) in [
            (
                "tool-missing",
                json!({"prompt":"missing", "subagent_type":"claudex-gpt-spark"}),
                None,
            ),
            (
                "tool-mismatch",
                json!({"prompt":"mismatch", "subagent_type":"claudex-gpt-spark", "claudex_model":"gpt-5.6-sol"}),
                None,
            ),
            (
                "tool-selected",
                json!({"prompt":"selected", "subagent_type":"claudex-gpt-spark", "claudex_model":"gpt-5.3-codex-spark"}),
                Some("gpt-5.3-codex-spark"),
            ),
        ] {
            let (arguments, _) = prepare_arguments("Agent", tool_id, &arguments);
            let arguments = arguments.expect("routed Agent intent");
            intents.record_from_user_messages(
                AgentEffortRecord {
                    client_user_id: None,
                    tool_name: "Agent",
                    tool_use_id: tool_id.to_owned(),
                    parent_model: "main-model",
                    arguments: &arguments,
                    user_messages: &[json!({
                        "role":"user",
                        "content":format!("implement this\n{routing}")
                    })],
                    system: &json!(null),
                },
                None,
            );
            let intent = intents.take(&request_without_user_id(
                arguments["prompt"].as_str().expect("correlated prompt"),
            ));
            assert_eq!(intent.model_override.as_deref(), expected);
        }
    }

    #[test]
    fn canonicalizes_model_and_effort_as_one_route_tuple() {
        let intents = AgentEffortIntents::default();
        let routing = r#"Claudex routing for this turn: {"providers":{},"selected_workers":[{"agent":"general-purpose","model":"main-model","effort":"high"}]} mandatory policy"#;
        let user_messages = [json!({
            "role":"user",
            "content":format!("Use the main-model worker for this task.\n{routing}")
        })];
        let mut routed = json!({
            "subagent_type":"general-purpose",
            "prompt":"same route",
            "claudex_model":"main-model",
            "claudex_effort":"xhigh"
        });
        super::super::agent_routing::hydrate_routing_fields_from_context(
            &mut routed,
            &user_messages,
            &json!(null),
            &crate::provider_config::ModelCatalog::default(),
        );
        assert_eq!(routed["claudex_model"], "main-model");
        assert_eq!(routed["claudex_effort"], "high");
        let (arguments, _) = prepare_arguments("Agent", "tool-same-model", &routed);
        let arguments = arguments.expect("same-model Agent intent");
        validate_routed_agent_arguments("Agent", &arguments, &user_messages, &json!(null))
            .expect("selected same-model worker is authorized");
        intents.record_from_user_messages(
            AgentEffortRecord {
                client_user_id: None,
                tool_name: "Agent",
                tool_use_id: "tool-same-model".to_owned(),
                parent_model: "main-model",
                arguments: &arguments,
                user_messages: &user_messages,
                system: &json!(null),
            },
            None,
        );

        let intent = intents.take(&request_without_user_id(
            arguments["prompt"].as_str().expect("correlated prompt"),
        ));
        assert_eq!(intent.model_override.as_deref(), Some("main-model"));
        assert!(!intent.model_is_inherited);
        assert_eq!(explicit(intent.effort), "high");
    }

    #[test]
    fn does_not_reauthorize_a_model_from_an_older_human_turn() {
        let intents = AgentEffortIntents::default();
        let (arguments, _) = prepare_arguments(
            "Task",
            "tool-resumed-model",
            &json!({
                "subagent_type":"general-purpose",
                "prompt":"resume research",
                "claudex_model":"grok-4.5",
                "claudex_effort":"high"
            }),
        );
        let arguments = arguments.expect("resumed Task intent");
        let user_messages = [
            json!({"role":"user","content":"Use grok-4.5 for this worker."}),
            json!({"role":"assistant","content":"The worker is continuing."}),
            json!({"role":"user","content":"continue"}),
        ];
        intents.record_from_user_messages(
            AgentEffortRecord {
                client_user_id: Some("resumed"),
                tool_name: "Task",
                tool_use_id: "tool-resumed-model".to_owned(),
                parent_model: "main-model",
                arguments: &arguments,
                user_messages: &user_messages,
                system: &json!(null),
            },
            None,
        );

        let mut request = request("resumed", "continue", true);
        request.messages = vec![
            user_messages[0].clone(),
            user_messages[1].clone(),
            json!({
                "role":"user",
                "content":format!("continue\n{}", arguments["prompt"])
            }),
        ];
        let intent = intents.take(&request);
        assert!(intent.matched);
        assert_eq!(intent.model_override, None);
        assert_eq!(explicit(intent.effort), "high");
    }

    #[test]
    fn rejects_invalid_agent_launch_intents_before_recording_routing_metadata() {
        let user_messages = [json!({
            "role":"user",
            "content":"Run the selected worker"
        })];
        let cases = [
            (
                json!({"subagent_type":"general-purpose","prompt":"missing model"}),
                "missing required `claudex_model`",
            ),
            (
                json!({"subagent_type":"general-purpose","prompt":"empty model","claudex_model":""}),
                "missing required `claudex_model`",
            ),
            (
                json!({"subagent_type":"general-purpose","prompt":"wrong model","claudex_model":"not-selected"}),
                "does not match the exact route",
            ),
            (
                json!({"subagent_type":"general-purpose","prompt":"non-string model","claudex_model":7}),
                "missing required `claudex_model`",
            ),
        ];
        for (arguments, message) in cases {
            let error =
                validate_routed_agent_arguments("Agent", &arguments, &user_messages, &json!(null))
                    .expect_err("invalid Agent launch must be rejected");
            assert!(error.to_string().contains(message), "{error}");
        }
        validate_routed_agent_arguments(
            "Read",
            &json!({"path":"README.md"}),
            &user_messages,
            &json!(null),
        )
        .expect("non-Agent tools do not require routing metadata");
    }

    #[test]
    fn authorizes_only_the_configured_custom_advisor_model() {
        let routing = r#"Claudex routing for this turn: {"providers":{},"selected_workers":[],"advisor":{"agent":"custom-advisor","model":"claude-fable-5","effort":"xhigh"},"custom_advisor_enabled":true} mandatory policy"#;
        let messages = [json!({
            "role":"user",
            "content":format!("Review this decision\n{routing}")
        })];

        assert!(
            validate_routed_agent_arguments(
                "Agent",
                &json!({
                    "subagent_type":"custom-advisor",
                    "claudex_model":"claude-fable-5"
                }),
                &messages,
                &json!(null),
            )
            .is_ok()
        );

        for rejected in [
            json!({
                "subagent_type":"custom-advisor",
                "claudex_model":"claude-sonnet-5"
            }),
            json!({
                "subagent_type":"general-purpose",
                "claudex_model":"claude-fable-5"
            }),
        ] {
            assert!(
                validate_routed_agent_arguments("Task", &rejected, &messages, &json!(null),)
                    .is_err()
            );
        }

        let disabled_routing = r#"Claudex routing for this turn: {"providers":{},"selected_workers":[],"advisor":{"agent":"custom-advisor","model":"claude-fable-5","effort":"xhigh"},"custom_advisor_enabled":false} mandatory policy"#;
        let disabled_messages = [json!({
            "role":"user",
            "content":format!("Use claude-fable-5 for this review.
        {disabled_routing}")
        })];
        assert!(
            validate_routed_agent_arguments(
                "Agent",
                &json!({
                    "subagent_type":"custom-advisor",
                    "claudex_model":"claude-fable-5"
                }),
                &disabled_messages,
                &json!(null),
            )
            .is_err()
        );
    }

    #[test]
    fn validates_agent_model_against_the_latest_route_or_active_user_literal() {
        let old = r#"Claudex routing for this turn: {"providers":{},"selected_workers":[{"agent":"claudex-gpt-spark","model":"gpt-old"}]} mandatory policy"#;
        let latest = r#"Claudex routing for this turn: {"providers":{"codex":{"agent":"claudex-gpt-spark","model":"gpt-5.3-codex-spark","model_prefixes":["gpt-"]}},"selected_workers":[{"agent":"claudex-gpt-spark","model":"gpt-5.3-codex-spark"}]} mandatory policy"#;
        let messages = [
            json!({"role":"assistant","content":old}),
            json!({"role":"user","content":format!("implement this\n{latest}")}),
            json!({"role":"assistant","content":old}),
        ];
        assert!(
            validate_routed_agent_arguments(
                "Agent",
                &json!({"subagent_type":"claudex-gpt-spark","claudex_model":"gpt-5.3-codex-spark"}),
                &messages,
                &json!(null),
            )
            .is_ok()
        );
        for rejected in [
            json!({"subagent_type":"claudex-gpt-spark"}),
            json!({"subagent_type":"claudex-gpt-spark","claudex_model":"gpt-old"}),
            json!({"subagent_type":"claude-code-guide","claudex_model":"claudex-gpt-spark"}),
        ] {
            assert!(
                validate_routed_agent_arguments("Agent", &rejected, &messages, &json!(null),)
                    .is_err()
            );
        }

        let explicit = [json!({
            "role":"user",
            "content":format!("Use gpt-5.6-sol for this worker.\n{latest}")
        })];
        assert!(
            validate_routed_agent_arguments(
                "Task",
                &json!({"subagent_type":"claudex-gpt-spark","claudex_model":"gpt-5.6-sol"}),
                &explicit,
                &json!(null),
            )
            .is_ok()
        );

        let compound = [json!({
            "role":"user",
            "content":"Use vendor@beta+1 for this worker."
        })];
        assert!(
            validate_routed_agent_arguments(
                "Agent",
                &json!({"subagent_type":"general-purpose","claudex_model":"beta"}),
                &compound,
                &json!(null),
            )
            .is_err()
        );
    }

    #[test]
    fn accepts_selected_worker_model_when_claude_uses_a_generic_agent_type() {
        let system = json!([{
            "type":"text",
            "text":"Claudex routing for this turn: {\"providers\":{},\"selected_agents\":[\"claudex-deepseek-flash\"],\"selected_workers\":[{\"agent\":\"claudex-deepseek-flash\",\"model\":\"opencode-go/deepseek-v4-flash\",\"effort\":\"high\"}]} mandatory policy"
        }]);
        let messages = [json!({"role":"user","content":"Please run this task"})];
        assert!(validate_routed_agent_arguments(
            "Agent",
            &json!({"subagent_type":"general-purpose","claudex_model":"opencode-go/deepseek-v4-flash"}),
            &messages,
            &system,
        )
        .is_ok());
        assert!(validate_routed_agent_arguments(
            "Agent",
            &json!({"subagent_type":"general-purpose","claudex_model":"opencode-go/deepseek-v4-pro"}),
            &messages,
            &system,
        )
        .is_err());
    }

    #[test]
    fn accepts_a_worker_snapshot_retained_in_the_transcript_after_compaction() {
        let routing = r#"Claudex routing for this turn: {"providers":{},"selected_workers":[{"agent":"claudex-grok","model":"grok-4.5","effort":"medium"}]} mandatory policy"#;
        let messages = [
            json!({"role":"assistant","content":routing}),
            json!({"role":"user","content":"Continue the research"}),
        ];
        assert!(
            validate_routed_agent_arguments(
                "Agent",
                &json!({"subagent_type":"general-purpose","claudex_model":"grok-4.5"}),
                &messages,
                &json!(null),
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_a_model_authorized_only_by_an_older_human_turn() {
        let messages = [
            json!({"role":"user","content":"Use grok-4.5 for the research SubAgent"}),
            json!({"role":"assistant","content":"I will continue"}),
            json!({"role":"user","content":"continue"}),
        ];
        assert!(
            validate_routed_agent_arguments(
                "Task",
                &json!({"subagent_type":"general-purpose","claudex_model":"grok-4.5"}),
                &messages,
                &json!(null),
            )
            .is_err()
        );
    }

    #[test]
    fn preserves_native_effort_and_non_agent_schemas() {
        let (_, public) =
            prepare_arguments("Agent", "tool", &json!({"prompt":"task","effort":"high"}));
        assert_eq!(public["effort"], "high");
        assert_eq!(
            tool_schema("Read", json!({"type":"object"})),
            json!({"type":"object"})
        );
        assert_eq!(
            tool_schema(
                "Agent",
                json!({"type":"object","properties":{},"required":"invalid"}),
            ),
            json!({"type":"object","properties":{},"required":"invalid"})
        );
    }

    #[test]
    fn rejects_non_agents_invalid_efforts_and_unmatched_requests() {
        let intents = AgentEffortIntents::default();
        intents.record(
            Some("session"),
            "Read",
            "read".to_owned(),
            "main-model",
            &json!({"prompt":"ignored"}),
        );
        intents.record(
            Some("session"),
            "Agent",
            "invalid".to_owned(),
            "main-model",
            &json!({"prompt":"task","claudex_effort":"invalid"}),
        );
        assert!(matches!(
            intents.take(&request("other", "different", true)).effort,
            AgentEffort::Unmatched
        ));
        assert!(matches!(
            intents.take(&request("session", "task", true)).effort,
            AgentEffort::ConfiguredDefault
        ));
        let (internal, public) = prepare_arguments("Read", "read", &json!({"path":"file"}));
        assert!(internal.is_none());
        assert_eq!(public, json!({"path":"file"}));
    }

    #[test]
    fn bounds_pending_intents_and_removes_completed_tools() {
        let intents = AgentEffortIntents::default();
        for index in 0..=super::MAX_PENDING_INTENTS {
            intents.record(
                Some("session"),
                "Agent",
                format!("tool-{index}"),
                "main-model",
                &json!({"prompt":format!("task-{index}")}),
            );
        }
        assert!(matches!(
            intents.take(&request("session", "task-0", true)).effort,
            AgentEffort::Unmatched
        ));
        intents.remove_tool_results(["tool-1", "missing"].into_iter());
        assert!(matches!(
            intents.take(&request("session", "task-1", true)).effort,
            AgentEffort::Unmatched
        ));
        assert!(matches!(
            intents.take(&request("session", "task-2", true)).effort,
            AgentEffort::ConfiguredDefault
        ));
    }

    #[test]
    fn bounds_correlated_intents_without_blocking_new_fanout() {
        let intents = AgentEffortIntents::default();
        let mut first_prompt = String::new();
        let mut second_prompt = String::new();
        for index in 0..=super::MAX_PENDING_INTENTS {
            let tool_id = format!("tool-correlated-{index}");
            let (internal, _) = prepare_arguments(
                "Agent",
                &tool_id,
                &json!({"prompt":format!("task-{index}")}),
            );
            let internal = internal.expect("correlated intent");
            capture_initial_prompts(index, &internal, &mut first_prompt, &mut second_prompt);
            intents.record(None, "Agent", tool_id, "main-model", &internal);
        }

        assert_eq!(
            intents.pending.lock().unwrap().len(),
            super::MAX_PENDING_INTENTS
        );
        assert!(matches!(
            intents.take(&request_without_user_id(&first_prompt)).effort,
            AgentEffort::Unmatched
        ));
        assert!(matches!(
            intents
                .take(&request_without_user_id(&second_prompt))
                .effort,
            AgentEffort::ConfiguredDefault
        ));
    }

    fn capture_initial_prompts(
        index: usize,
        internal: &Value,
        first_prompt: &mut String,
        second_prompt: &mut String,
    ) {
        match index {
            0 => *first_prompt = internal["prompt"].as_str().unwrap().to_owned(),
            1 => *second_prompt = internal["prompt"].as_str().unwrap().to_owned(),
            _ => {}
        }
    }

    #[test]
    fn preserves_invalid_and_preconfigured_agent_schemas_exactly() {
        assert_eq!(tool_schema("Agent", json!(null)), json!(null));
        assert_eq!(
            tool_schema("Agent", json!({"properties":"invalid"})),
            json!({"properties":"invalid"})
        );
        let existing = json!({
            "properties":{
                "claudex_effort":{"type":"string","const":"high"},
                "claudex_model":{"type":"string","const":"grok-4.5"}
            }
        });
        assert_eq!(tool_schema("Agent", existing.clone()), existing);
        let already_required = json!({
            "properties":{"claudex_model":{"type":"string"}},
            "required":["claudex_model"]
        });
        assert_eq!(
            tool_schema("Agent", already_required.clone()),
            already_required
        );
    }

    #[test]
    fn exercises_non_object_context_and_advisor_routing_boundaries() {
        let mut scalar = json!("not an object");
        super::super::agent_routing::hydrate_routing_fields_from_context(
            &mut scalar,
            &[],
            &json!(null),
            &crate::provider_config::ModelCatalog::default(),
        );
        assert_eq!(scalar, json!("not an object"));

        let mut scalar_standard = json!("not an object");
        super::super::agent_routing::hydrate_standard_agent_to_parent(
            &mut scalar_standard,
            "parent-model",
        );
        assert_eq!(scalar_standard, json!("not an object"));

        let routing = r#"Claudex routing for this turn: {"providers":{},"selected_workers":[],"advisor":{"agent":"custom-advisor","model":"advisor-model","effort":"high"},"custom_advisor_enabled":true} mandatory policy"#;
        let messages = [json!({"role":"user", "content":routing})];
        let arguments = json!({"subagent_type":"custom-advisor"});
        assert!(super::super::agent_routing::model_is_authorized(
            &arguments,
            &messages,
            &json!(null),
            "advisor-model"
        ));
        assert!(!super::super::agent_routing::model_is_authorized(
            &arguments,
            &messages,
            &json!(null),
            "different-model"
        ));
    }

    #[test]
    fn custom_advisor_enablement_is_required_only_when_explicitly_disabled() {
        let disabled = [json!({
            "role": "user",
            "content": r#"Claudex routing for this turn: {"providers":{},"advisor":{"agent":"custom-advisor","model":"advisor-model","effort":"high"},"custom_advisor_enabled":false}"#
        })];
        let arguments = json!({"subagent_type":"custom-advisor"});
        assert!(!super::super::agent_routing::model_is_authorized(
            &arguments,
            &disabled,
            &json!(null),
            "advisor-model"
        ));

        let omitted_flag = [json!({
            "role": "user",
            "content": r#"Claudex routing for this turn: {"providers":{},"advisor":{"agent":"custom-advisor","model":"advisor-model","effort":"high"}}"#
        })];
        assert!(super::super::agent_routing::model_is_authorized(
            &arguments,
            &omitted_flag,
            &json!(null),
            "advisor-model"
        ));
    }

    #[test]
    fn retains_terminal_intents_only_for_unfinished_matching_sessions() {
        use std::collections::HashSet;

        let intent = super::AgentEffortIntent {
            client_user_id: Some("user".to_owned()),
            prompt: "work".to_owned(),
            correlated: true,
            effort: None,
            model_override: None,
            model_is_inherited: false,
            run_in_background: false,
            tool_use_id: "tool".to_owned(),
            created_at: Instant::now(),
            created_unix_seconds: 0,
        };
        assert!(super::retain_terminal_intent(
            &intent,
            &HashSet::new(),
            Some("user")
        ));
        assert!(super::retain_terminal_intent(
            &intent,
            &HashSet::from(["other".to_owned()]),
            Some("user")
        ));
        assert!(super::retain_terminal_intent(
            &intent,
            &HashSet::from(["tool".to_owned()]),
            Some("other")
        ));
        assert!(!super::retain_terminal_intent(
            &intent,
            &HashSet::from(["tool".to_owned()]),
            Some("user")
        ));
        let mut uncorrelated = intent;
        uncorrelated.correlated = false;
        assert!(super::retain_terminal_intent(
            &uncorrelated,
            &HashSet::from(["tool".to_owned()]),
            Some("user")
        ));
    }

    #[test]
    fn session_id_header_does_not_hide_a_live_muse_spark_launch() {
        let mut request: MessagesRequest = serde_json::from_value(json!({
            "model":"meta/muse-spark-1.2-contributor",
            "system":"ordinary system",
            "messages":[{"role":"user","content":"Within horse-racing-data, run the pooler GUC checks.\n\nclaudex_launch_id: toolu_9472e7fc33464570a065847cb744fedc\nclaudex_model: meta/muse-spark-1.2-contributor\n\n<claudex-agent-id>toolu_9472e7fc33464570a065847cb744fedc</claudex-agent-id>"}]
        }))
        .expect("request");
        super::super::RequestIdentity::new(Some("session-child".to_owned()), None, None)
            .attach(&mut request);

        assert!(super::is_subagent_request(&request));
    }

    #[test]
    fn session_id_header_does_not_hide_cc_is_subagent_billing() {
        let mut request: MessagesRequest = serde_json::from_value(json!({
            "model":"meta/muse-spark-1.2-contributor",
            "system":"cc_is_subagent=true",
            "messages":[{"role":"user","content":"ordinary delegated task"}]
        }))
        .expect("request");
        super::super::RequestIdentity::new(Some("session-child".to_owned()), None, None)
            .attach(&mut request);

        assert!(super::is_subagent_request(&request));
    }

    #[test]
    fn standard_agent_hydration_skips_claudex_workers_and_wrong_advisors() {
        let mut routed = json!({"subagent_type":"claudex-worker"});
        super::super::agent_routing::hydrate_standard_agent_to_parent(&mut routed, "parent-model");
        assert!(routed.get("claudex_model").is_none());

        let arguments = json!({"subagent_type":"custom-advisor"});
        let messages = [json!({
            "role": "user",
            "content": r#"Claudex routing for this turn: {"providers":{},"advisor":{"agent":"custom-advisor","model":"advisor-model","effort":"high"},"custom_advisor_enabled":true}"#
        })];
        assert!(!super::super::agent_routing::model_is_authorized(
            &arguments,
            &messages,
            &json!(null),
            "wrong-model"
        ));
    }

    fn request_without_user_id(prompt: &str) -> MessagesRequest {
        let mut request = request("ignored", prompt, true);
        request.metadata = json!({});
        request
    }
}
