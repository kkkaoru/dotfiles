use serde_json::Value;

use crate::anthropic::agent_route_validation::BlockedSubagentError;

pub(super) fn note_reused_subagent_launch(
    context: &crate::anthropic::subscription::SubscriptionToolContext,
    name: &str,
    routed_input: &mut Value,
) {
    let Some(session_id) = context.session_id.as_deref() else {
        return;
    };
    let Some(recipient) = context
        .subagent_reuse
        .rewrite_launch_input(session_id, routed_input)
    else {
        return;
    };
    tracing::info!(
        session_id,
        recipient,
        tool = name,
        "subscription Agent/Task launch reused an existing SubAgent"
    );
}

pub(super) fn reject_unavailable_subagent_model(
    context: &crate::anthropic::subscription::SubscriptionToolContext,
    routed_input: &Value,
) -> std::result::Result<(), BlockedSubagentError> {
    let Some(model) = crate::anthropic::agent_effort::requested_model(routed_input) else {
        return Ok(());
    };
    if crate::subagent_policy::model_is_disabled(&context.disabled_subagent_models, model) {
        return Err(BlockedSubagentError::policy_disabled(model));
    }
    let exhausted = crate::anthropic::agent_routing::routing_disables_subagent_model(
        &context.user_messages,
        &context.system,
        model,
    ) || context.launch_model_is_exhausted(model);
    if exhausted {
        return Err(BlockedSubagentError::cooldown(model));
    }
    Ok(())
}

pub(super) fn record_prepared_agent_intent(
    context: &crate::anthropic::subscription::SubscriptionToolContext,
    name: &str,
    id: &str,
    intent: Option<&Value>,
) {
    let Some(intent) = intent else {
        return;
    };
    context.agent_efforts.record_from_user_messages(
        crate::anthropic::agent_effort::AgentEffortRecord {
            client_user_id: context.client_user_id.as_deref(),
            tool_name: name,
            tool_use_id: id.to_owned(),
            parent_model: &context.parent_model,
            arguments: intent,
            user_messages: &context.user_messages,
            system: &context.system,
        },
        Some(&context.model_catalog),
    );
}
