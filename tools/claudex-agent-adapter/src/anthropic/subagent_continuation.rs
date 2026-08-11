use super::{
    Bridge, MessagesRequest,
    agent_effort::{AgentEffortIntents, AgentIntent, is_subagent_request},
    content::collect_turn_tool_results,
};

impl Bridge {
    /// Recover a routed worker's launch intent while it returns a Claude Code tool result.
    ///
    /// Claude Code's continuation may omit the original launch marker from its request, but the
    /// matching adapter session retains the original transcript. Resolve only through a pending
    /// tool ID owned by that session; this never guesses between concurrent SubAgents.
    pub(super) async fn subagent_tool_continuation(
        &self,
        request: &MessagesRequest,
    ) -> Option<AgentIntent> {
        if !is_subagent_request(request) {
            return None;
        }
        let results = {
            let results = collect_turn_tool_results(&request.messages);
            (!results.is_empty()).then_some(results)?
        };
        let session = self.find_result_session(&results).await?;
        let transcript = session.transcript.lock().await.clone();
        correlated_intent(&self.agent_efforts, request, transcript)
    }
}

fn correlated_intent(
    intents: &AgentEffortIntents,
    request: &MessagesRequest,
    transcript: Vec<serde_json::Value>,
) -> Option<AgentIntent> {
    if transcript.is_empty() {
        return None;
    }
    let mut correlated = request.clone();
    correlated.messages = transcript;
    let intent = intents.take(&correlated);
    intent.matched.then_some(intent)
}

#[cfg(test)]
// Coverage gates measure production code; this module only supplies direct fixtures.
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::{AgentEffortIntents, MessagesRequest, correlated_intent};
    use crate::anthropic::{AgentEffortRecord, agent_effort::AgentEffort};

    #[test]
    fn preserves_a_routed_agent_intent_for_its_tool_continuation() {
        let intents = AgentEffortIntents::default();
        let prompt = "research\nclaudex_launch_id: tool-agent\n<claudex-agent-id>tool-agent</claudex-agent-id>";
        let user_messages = [json!({"role":"user","content":"Use gpt-worker."})];
        intents.record_from_user_messages(
            AgentEffortRecord {
                client_user_id: Some("outer"),
                tool_name: "Agent",
                tool_use_id: "tool-agent".to_owned(),
                parent_model: "main",
                arguments: &json!({"prompt":prompt,"claudex_model":"gpt-worker","claudex_effort":"xhigh"}),
                user_messages: &user_messages,
                system: &json!(null),
            },
            None,
        );
        let request = MessagesRequest {
            system: json!("cc_is_subagent=true"),
            messages: vec![
                json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call-search","content":"result"}]}),
            ],
            metadata: json!({"user_id":"outer"}),
            model: "main".to_owned(),
            tools: Vec::new(),
            stream: false,
            output_config: json!({}),
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        };
        let intent = correlated_intent(
            &intents,
            &request,
            vec![json!({"role":"user","content":prompt})],
        )
        .expect("correlates retained launch transcript");
        assert!(intent.is_subagent && intent.matched);
        assert_eq!(intent.model_override.as_deref(), Some("gpt-worker"));
        assert!(matches!(intent.effort, AgentEffort::Explicit(ref value) if value == "xhigh"));
        assert!(
            correlated_intent(&intents, &request, Vec::new()).is_none(),
            "empty retained transcripts must not invent a continuation intent"
        );
    }
}
