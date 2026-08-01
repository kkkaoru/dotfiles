use serde_json::Value;

use super::super::{Bridge, MessagesRequest, agent_effort::AgentEffort};

pub(in crate::anthropic) fn request_effort(output_config: &Value) -> Option<&str> {
    output_config
        .get("effort")
        .and_then(Value::as_str)
        .filter(|effort| valid_effort(effort))
}

pub(in crate::anthropic) fn valid_effort(effort: &str) -> bool {
    matches!(effort, "low" | "medium" | "high" | "xhigh" | "max")
}

impl Bridge {
    #[allow(clippy::excessive_nesting)]
    pub(in crate::anthropic) fn resolve_request_effort(
        &self,
        request: &MessagesRequest,
        agent_effort: AgentEffort,
    ) -> Option<String> {
        let launch_scoped = self.app.launch_scoped_effort(&request.model);
        let configured = launch_scoped
            .as_deref()
            .or_else(|| self.model_catalog.worker_effort_for_model(&request.model));
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
            AgentEffort::Unmatched => {
                let requested = request_effort(&request.output_config);
                if let Some(launch_scoped) = launch_scoped {
                    if let Some(requested) =
                        requested.filter(|requested| *requested != launch_scoped)
                    {
                        tracing::warn!(
                            effort = requested,
                            configured = %launch_scoped,
                            model = %request.model,
                            "normalizing request effort to the launch-scoped provider route"
                        );
                    }
                    Some(launch_scoped)
                } else {
                    requested
                        .map(str::to_owned)
                        .or_else(|| configured.map(str::to_owned))
                        .or_else(|| self.claude_effort())
                }
            }
        }
    }
}
