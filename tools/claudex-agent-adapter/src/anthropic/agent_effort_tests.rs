#[cfg(test)]
// Coverage gates measure production code; test implementations are excluded.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::{AgentEffort, AgentEffortIntents, prepare_arguments, tool_schema};
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
            &json!({"prompt":"task-a","effort":"high"}),
        );
        assert!(matches!(
            intents.take(&request("session-a", "task-a", false)),
            AgentEffort::Unmatched
        ));
        assert_eq!(
            explicit(intents.take(&request("session-a", "task-a", true))),
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
            &json!({"prompt":"same","effort":"high"}),
        );
        intents.record(
            Some("session-a"),
            "Agent",
            "tool-a2".to_owned(),
            &json!({"prompt":"same","effort":"low"}),
        );
        intents.record(
            Some("session-b"),
            "Agent",
            "tool-b".to_owned(),
            &json!({"prompt":"same","effort":"medium"}),
        );
        assert_eq!(
            explicit(intents.take(&request("session-b", "same", true))),
            "medium"
        );
        assert_eq!(
            explicit(intents.take(&request("session-a", "same", true))),
            "high"
        );
        assert_eq!(
            explicit(intents.take(&request("session-a", "same", true))),
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
            first.as_ref().expect("first intent"),
        );
        intents.record(
            Some("outer-session"),
            "Agent",
            "tool-second".to_owned(),
            second.as_ref().expect("second intent"),
        );
        let first = first.expect("first intent");
        let second = second.expect("second intent");
        let second_prompt = second["prompt"].as_str().expect("second prompt");
        let first_prompt = first["prompt"].as_str().expect("first prompt");
        assert_eq!(
            explicit(intents.take(&request_without_user_id(second_prompt))),
            "low"
        );
        assert_eq!(
            explicit(intents.take(&request_without_user_id(first_prompt))),
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
            &json!({"prompt":"task"}),
        );
        assert!(matches!(
            intents.take(&request("session", "task", true)),
            AgentEffort::ConfiguredDefault
        ));
    }

    #[test]
    fn adds_and_strips_adapter_only_agent_effort() {
        let schema = tool_schema("Agent", json!({"type":"object"}));
        assert_eq!(
            schema["properties"]["claudex_effort"]["enum"],
            json!(["low", "medium", "high", "xhigh", "max"])
        );
        let (internal, public) = prepare_arguments(
            "Agent",
            "tool-mid",
            &json!({"prompt":"task","claudex_effort":"mid"}),
        );
        let internal = internal.expect("agent intent");
        assert_eq!(internal["claudex_effort"], "mid");
        assert!(public.get("claudex_effort").is_none());

        let intents = AgentEffortIntents::default();
        intents.record(None, "Agent", "tool-mid".to_owned(), &internal);
        assert_eq!(
            explicit(intents.take(&request_without_user_id(
                internal["prompt"].as_str().expect("correlated prompt")
            ))),
            "medium"
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
    }

    #[test]
    fn rejects_non_agents_invalid_efforts_and_unmatched_requests() {
        let intents = AgentEffortIntents::default();
        intents.record(
            Some("session"),
            "Read",
            "read".to_owned(),
            &json!({"prompt":"ignored"}),
        );
        intents.record(
            Some("session"),
            "Agent",
            "invalid".to_owned(),
            &json!({"prompt":"task","claudex_effort":"invalid"}),
        );
        assert!(matches!(
            intents.take(&request("other", "different", true)),
            AgentEffort::Unmatched
        ));
        assert!(matches!(
            intents.take(&request("session", "task", true)),
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
                &json!({"prompt":format!("task-{index}")}),
            );
        }
        assert!(matches!(
            intents.take(&request("session", "task-0", true)),
            AgentEffort::Unmatched
        ));
        intents.remove_tool_results(["tool-1", "missing"].into_iter());
        assert!(matches!(
            intents.take(&request("session", "task-1", true)),
            AgentEffort::Unmatched
        ));
        assert!(matches!(
            intents.take(&request("session", "task-2", true)),
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
            "properties":{"claudex_effort":{"type":"string","const":"high"}}
        });
        assert_eq!(tool_schema("Agent", existing.clone()), existing);
    }

    fn request_without_user_id(prompt: &str) -> MessagesRequest {
        let mut request = request("ignored", prompt, true);
        request.metadata = json!({});
        request
    }
}
