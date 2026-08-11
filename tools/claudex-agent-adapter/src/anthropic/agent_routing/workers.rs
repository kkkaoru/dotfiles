use super::super::subscription::valid_effort;
use super::{ADAPTER_MODEL, active_routing_summary};
use serde_json::Value;

pub(in crate::anthropic) fn configured_worker_fields(
    arguments: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
) -> Option<(String, Option<String>)> {
    let (model, effort) = model_catalog.worker_fields(arguments.get("subagent_type")?.as_str()?)?;
    Some((model.to_owned(), Some(effort.to_owned())))
}

pub(in crate::anthropic) fn generic_worker_fields(
    arguments: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
) -> Option<(String, Option<String>)> {
    if arguments.get(ADAPTER_MODEL).is_some()
        || !is_generic_agent_type(arguments.get("subagent_type").and_then(Value::as_str))
    {
        return None;
    }
    model_catalog
        .default_worker_fields()
        .map(|(model, effort)| (model.to_owned(), Some(effort.to_owned())))
}

pub(in crate::anthropic) fn generic_worker_model_matches(
    arguments: &Value,
    model_catalog: &crate::provider_config::ModelCatalog,
    model: &str,
) -> bool {
    is_generic_agent_type(arguments.get("subagent_type").and_then(Value::as_str))
        && model_catalog
            .default_worker_fields()
            .is_some_and(|(configured, _)| configured == model)
}

pub(in crate::anthropic) fn is_generic_agent_type(subagent_type: Option<&str>) -> bool {
    subagent_type.is_none()
        || subagent_type
            .map(str::trim)
            .is_some_and(|agent| matches!(agent, "Explore" | "general-purpose"))
}

pub(in crate::anthropic) fn selected_worker_fields(
    arguments: &Value,
    messages: &[Value],
    system: &Value,
) -> Option<(String, Option<String>)> {
    let summary = active_routing_summary(messages, system)?;
    let workers = summary.get("selected_workers")?.as_array()?;
    let agent = arguments.get("subagent_type").and_then(Value::as_str);
    let worker = agent
        .and_then(|agent| {
            workers
                .iter()
                .find(|worker| worker.get("agent").and_then(Value::as_str) == Some(agent))
        })
        .or_else(|| {
            is_generic_agent_type(agent)
                .then(|| default_subagent_worker(&summary, workers))
                .flatten()
        })?;
    Some((
        worker.get("model")?.as_str()?.to_owned(),
        worker
            .get("effort")
            .and_then(Value::as_str)
            .filter(|effort| valid_effort(effort))
            .map(str::to_owned),
    ))
}

pub(in crate::anthropic) fn default_subagent_worker<'a>(
    summary: &'a Value,
    workers: &'a [Value],
) -> Option<&'a Value> {
    let route = summary.get("default_subagent_route");
    route
        .and_then(|route| {
            let route_agent = route.get("agent").and_then(Value::as_str)?;
            let route_model = route.get("model").and_then(Value::as_str)?;
            workers.iter().find(|worker| {
                worker.get("agent").and_then(Value::as_str) == Some(route_agent)
                    && worker.get("model").and_then(Value::as_str) == Some(route_model)
            })
        })
        .or_else(|| {
            workers
                .iter()
                .find(|worker| worker.get("model").and_then(Value::as_str).is_some())
        })
}
