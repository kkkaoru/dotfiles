use anyhow::{Context, Result};
use serde_json::Value;

use super::SubscriptionStream;
use super::launch_prep::{
    note_reused_subagent_launch, record_prepared_agent_intent, reject_unavailable_subagent_model,
};

impl SubscriptionStream {
    #[cfg(test)]
    pub(in crate::anthropic) fn prepare_tool_input(
        &self,
        name: &str,
        id: &str,
        input: &Value,
    ) -> Result<Value> {
        Ok(self.route_agent_tool_input(name, id, input)?.1)
    }

    /// Returns `(private_routed, public)` so occupancy / reuse can see hydrated
    /// `claudex_model` before public schema stripping.
    pub(in crate::anthropic) fn route_agent_tool_input(
        &self,
        name: &str,
        id: &str,
        input: &Value,
    ) -> Result<(Value, Value)> {
        if !crate::anthropic::agent_effort::is_agent_tool(name) {
            let cloned = input.clone();
            return Ok((cloned.clone(), cloned));
        }
        let context = self
            .tool_context
            .as_ref()
            .context("subscription Agent/Task call has no routing context")?;
        let mut routed_input = input.clone();
        crate::anthropic::agent_routing::hydrate_routing_fields_from_context(
            &mut routed_input,
            &context.user_messages,
            &context.system,
            &context.model_catalog,
        );
        crate::anthropic::agent_routing::hydrate_standard_agent_to_parent(
            &mut routed_input,
            &context.parent_model,
        );
        note_reused_subagent_launch(context, name, &mut routed_input);
        reject_unavailable_subagent_model(context, &routed_input)?;
        if routed_input.get("claudex_model").is_none() {
            tracing::warn!(
                tool = name,
                subagent_type = ?routed_input.get("subagent_type"),
                native_model = ?routed_input.get("model"),
                "subscription Agent/Task omitted Claudex routing fields"
            );
        }
        crate::anthropic::agent_effort::validate_routed_agent_arguments_with_catalog(
            name,
            &routed_input,
            &context.user_messages,
            &context.system,
            &context.model_catalog,
        )?;
        let (intent, public) = crate::anthropic::agent_effort::prepare_arguments_for_user(
            name,
            id,
            &routed_input,
            &context.user_messages,
            &context.system,
        );
        record_prepared_agent_intent(context, name, id, intent.as_ref());
        Ok((routed_input, public))
    }
}
