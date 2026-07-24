#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Instant;

    use serde_json::json;

    use super::{
        AgentEffort, AgentEffortIntents, prepare_arguments, prepare_arguments_for_user, tool_schema,
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
            claudex_collaborator_model: None,
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
        assert_eq!(intent.model_override.as_deref(), Some("main-model"));
        assert!(matches!(intent.effort, AgentEffort::ConfiguredDefault));
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

        assert_eq!(reused.model_override.as_deref(), Some("provider-model"));
        assert_eq!(explicit(reused.effort), "high");
        let pending = intents.pending.lock().unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending.front().unwrap().tool_use_id, "tool-second");
        assert_eq!(pending.back().unwrap().tool_use_id, "tool-reused");
    }

    #[test]
    fn adds_and_strips_adapter_only_agent_effort() {
        for tool_name in ["Agent", "Task"] {
            let schema = tool_schema(tool_name, json!({"type":"object"}));
            assert_eq!(
                schema["properties"]["claudex_effort"]["enum"],
                json!(["low", "medium", "high", "xhigh", "max"])
            );
            assert_eq!(
                schema["properties"]["claudex_model"]["type"],
                "string"
            );
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
            intents.record(
                None,
                tool_name,
                tool_use_id,
                "main-model",
                &internal,
            );
            assert_eq!(
                explicit(intents.take(&request_without_user_id(
                    internal["prompt"].as_str().expect("correlated prompt")
                )).effort),
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
        );
        let internal = internal.expect("Task routing intent");
        assert_eq!(internal["claudex_model"], "gpt-5.6-sol");
        assert_eq!(internal["claudex_effort"], "high");
        assert!(public.get("model").is_none());
        assert!(public.get("claudex_model").is_none());
        assert!(public.get("claudex_effort").is_none());
    }

    #[test]
    fn removes_invented_mailbox_names_but_preserves_user_supplied_names() {
        let arguments = json!({
            "prompt":"audit contracts", "name":"wf_contract_audit",
            "subagent_type":"general-purpose"
        });
        let ordinary = [json!({"role":"user","content":"Run a contract audit SubAgent"})];
        let (_, public) =
            prepare_arguments_for_user("Agent", "tool-ordinary", &arguments, &ordinary);
        assert!(public.get("name").is_none());
        let (_, public) = prepare_arguments_for_user(
            "Agent",
            "tool-coincidental",
            &json!({"prompt":"audit contracts", "name":"audit"}),
            &ordinary,
        );
        assert!(public.get("name").is_none());

        let explicit = [json!({
            "role":"user", "content":"Use the named teammate wf_contract_audit"
        })];
        let (_, public) =
            prepare_arguments_for_user("Agent", "tool-named", &arguments, &explicit);
        assert_eq!(public["name"], "wf_contract_audit");

        let stale = [
            json!({"role":"user","content":"Earlier I named wf_contract_audit"}),
            json!({"role":"user","content":"Run another ordinary SubAgent"}),
            json!({"role":"user","content":"<agent-message from=\"wf_contract_audit\">done</agent-message>"}),
        ];
        let (_, public) =
            prepare_arguments_for_user("Agent", "tool-stale", &arguments, &stale);
        assert!(public.get("name").is_none());

        let schema = tool_schema("Agent", json!({
            "type":"object", "properties":{"name":{"type":"string"}}
        }));
        assert!(
            schema["properties"]["name"]["description"]
                .as_str()
                .expect("name guidance")
                .contains("never invent one")
        );
    }

    #[test]
    fn resolves_parent_and_arbitrary_explicit_provider_models() {
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
        assert_eq!(intent.model_override.as_deref(), Some("parent-model"));

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
                None,
                "Agent",
                tool_id,
                "parent-model",
                &explicit,
                &user_messages,
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
            ("tool-omitted", "Run the commit command", "parent-model"),
            (
                "tool-prefix-only",
                "Use claude-sonnet-5-newer for this SubAgent",
                "parent-model",
            ),
            (
                "tool-dot-suffix",
                "Use claude-sonnet-5.1 for this SubAgent",
                "parent-model",
            ),
            (
                "tool-explicit",
                "Use claude-sonnet-5.",
                "claude-sonnet-5",
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
                None,
                "Agent",
                tool_id.to_owned(),
                "parent-model",
                &arguments,
                &user_messages,
            );
            let intent = intents.take(&request_without_user_id(
                arguments["prompt"].as_str().expect("correlated prompt"),
            ));
            assert_eq!(intent.model_override.as_deref(), Some(expected));
        }
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
            if index == 0 {
                first_prompt = internal["prompt"].as_str().unwrap().to_owned();
            } else if index == 1 {
                second_prompt = internal["prompt"].as_str().unwrap().to_owned();
            }
            intents.record(None, "Agent", tool_id, "main-model", &internal);
        }

        assert_eq!(intents.pending.lock().unwrap().len(), super::MAX_PENDING_INTENTS);
        assert!(matches!(
            intents.take(&request_without_user_id(&first_prompt)).effort,
            AgentEffort::Unmatched
        ));
        assert!(matches!(
            intents.take(&request_without_user_id(&second_prompt)).effort,
            AgentEffort::ConfiguredDefault
        ));
    }

    #[test]
    fn tolerates_invalid_and_preconfigured_agent_schemas() {
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
    }

    fn request_without_user_id(prompt: &str) -> MessagesRequest {
        let mut request = request("ignored", prompt, true);
        request.metadata = json!({});
        request
    }
}
