use std::collections::VecDeque;

use serde_json::Value;

use super::super::AgentEffortIntent;

pub(super) fn authorized_model(
    arguments: &Value,
    user_messages: &[Value],
    system: &Value,
    model_catalog: Option<&crate::provider_config::ModelCatalog>,
    model: &str,
) -> bool {
    match model_catalog {
        Some(catalog) => crate::anthropic::agent_routing::model_is_authorized_with_catalog(
            arguments,
            user_messages,
            system,
            catalog,
            model,
        ),
        None => crate::anthropic::agent_routing::model_is_authorized(
            arguments,
            user_messages,
            system,
            model,
        ),
    }
}

pub(super) fn unique_correlated_candidate(
    pending: &VecDeque<AgentEffortIntent>,
    client_user_id: Option<&str>,
) -> Option<usize> {
    let mut candidates = pending.iter().enumerate().filter(|(_, intent)| {
        intent.correlated && intent.client_user_id.as_deref() == client_user_id
    });
    let candidate = candidates.next()?;
    candidates.next().is_none().then_some(candidate.0)
}

pub(in crate::anthropic) fn retain_terminal_intent(
    intent: &AgentEffortIntent,
    terminal_ids: &std::collections::HashSet<String>,
    client_user_id: Option<&str>,
) -> bool {
    !intent.correlated
        || !terminal_ids.contains(intent.tool_use_id.as_str())
        || client_user_id.is_some_and(|id| intent.client_user_id.as_deref() != Some(id))
}
