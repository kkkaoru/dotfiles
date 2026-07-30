use super::super::{Bridge, MessagesRequest, agent_effort::AgentEffort};
use super::request_effort;

impl Bridge {
    pub(in crate::anthropic) fn resolve_request_effort(
        &self,
        request: &MessagesRequest,
        agent_effort: AgentEffort,
    ) -> Option<String> {
        let configured = self.model_catalog.worker_effort_for_model(&request.model);
        match agent_effort {
            AgentEffort::Explicit(effort) => configured.map_or_else(
                || Some(effort.clone()),
                |configured| {
                    if configured != effort {
                        tracing::warn!(%effort, %configured, model = %request.model, "normalizing SubAgent effort to the configured worker route");
                    }
                    Some(configured.to_owned())
                },
            ),
            AgentEffort::ConfiguredDefault => configured
                .map(str::to_owned)
                .or_else(|| self.claude_effort()),
            AgentEffort::Unmatched => request_effort(&request.output_config)
                .map(str::to_owned)
                .or_else(|| configured.map(str::to_owned))
                .or_else(|| self.claude_effort()),
        }
    }
}
