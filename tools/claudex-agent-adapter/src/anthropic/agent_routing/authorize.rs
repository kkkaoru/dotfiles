use serde_json::Value;

use super::{
    IMPLICIT_MODEL, expected_worker_fields, explicit, selected_worker_fields,
    summary::{
        active_routing_summary, advisor_launch_disabled, configured_advisor_model_matches,
    },
    workers::generic_worker_model_matches,
};

pub(in crate::anthropic) fn model_is_authorized(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
    model: &str,
) -> bool {
    // Claude Code can preserve its built-in `general-purpose` / `Explore` type while passing a
    // routed provider model. The selected model remains the authority; requiring its display
    // type to equal the configured worker name rejects that valid launch before ACP can start.
    if advisor_launch_disabled(arguments, messages, system) {
        return false;
    }
    explicit::active_user_requests_model(messages, model)
        || selected_worker_model_matches(arguments, messages, system, model)
        || configured_advisor_model_matches(arguments, messages, system, model)
}

pub(in crate::anthropic) fn model_is_authorized_with_catalog(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
    model: &str,
) -> bool {
    if advisor_launch_disabled(arguments, messages, system) {
        return false;
    }
    if let Some((expected, _)) = expected_worker_fields(arguments, messages, system, model_catalog)
    {
        return expected == model;
    }
    arguments.get(IMPLICIT_MODEL).and_then(Value::as_str) == Some(model)
        || model_is_authorized(arguments, messages, system, model)
        || generic_worker_model_matches(arguments, model_catalog, model)
}

pub(in crate::anthropic) fn routing_disables_subagent_model(
    messages: &[Value],
    system: &Value,
    model: &str,
) -> bool {
    active_routing_summary(messages, system).is_some_and(|summary| {
        super::super::routing_quota::summary_marks_model_exhausted(&summary, model)
    })
}

fn selected_worker_model_matches(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
    model: &str,
) -> bool {
    selected_worker_fields(arguments, messages, system)
        .is_some_and(|(selected, _)| selected == model)
}
