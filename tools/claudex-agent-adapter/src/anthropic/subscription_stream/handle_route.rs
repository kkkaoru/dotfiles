use anyhow::{Result, bail};
use serde_json::Value;

use super::SubscriptionStream;
use super::launch_prep::{
    note_reused_subagent_launch, record_prepared_agent_intent,
    reject_nested_or_capped_agent_launch, reject_unavailable_subagent_model,
};

pub(super) const SEND_MESSAGE_REQUIRES_TO: &str =
    "SendMessage requires `to` with the exact prior Agent/Task agentId; it was not executed.";

fn send_message_has_recipient(input: &Value) -> bool {
    input
        .get("to")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
}

fn route_non_agent_tool_input(name: &str, id: &str, input: &Value) -> Result<(Value, Value)> {
    if name == "SendMessage" {
        return route_send_message_input(id, input);
    }
    let cloned = input.clone();
    Ok((cloned.clone(), cloned))
}

fn route_send_message_input(id: &str, input: &Value) -> Result<(Value, Value)> {
    if !send_message_has_recipient(input) {
        bail!(SEND_MESSAGE_REQUIRES_TO);
    }
    let public = crate::anthropic::agent_effort::prepare_arguments_for_user(
        "SendMessage",
        id,
        input,
        &[],
        &Value::Null,
    )
    .1;
    Ok((input.clone(), public))
}

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
            return route_non_agent_tool_input(name, id, input);
        }
        let context = self.tool_context.as_ref().ok_or_else(|| {
            anyhow::Error::new(
                crate::anthropic::agent_route_validation::BlockedSubagentError::missing_config(
                    "subscription Agent/Task call has no routing context",
                ),
            )
        })?;
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
            &context.model_catalog,
        );
        note_reused_subagent_launch(context, name, &mut routed_input);
        if crate::anthropic::subagent_reuse::is_send_message_follow_up(&routed_input) {
            let public = crate::anthropic::agent_effort::prepare_arguments_for_user(
                "SendMessage",
                id,
                &routed_input,
                &context.user_messages,
                &context.system,
            )
            .1;
            return Ok((routed_input, public));
        }
        reject_nested_or_capped_agent_launch(context, &routed_input)?;
        reject_unavailable_subagent_model(context, &routed_input)?;
        if routed_input.get("claudex_model").is_none() {
            tracing::warn!(
                tool = name,
                subagent_type = ?routed_input.get("subagent_type"),
                native_model = ?routed_input.get("model"),
                "subscription Agent/Task omitted Claudex routing fields"
            );
        }
        crate::anthropic::agent_effort::validate_routed_agent_arguments_with_reason(
            name,
            &routed_input,
            &context.user_messages,
            &context.system,
            &context.model_catalog,
        )
        .map_err(anyhow::Error::new)?;
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
