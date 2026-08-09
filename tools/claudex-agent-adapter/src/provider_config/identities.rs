use super::Provider;
use crate::agent_backend::BackendRoute;

pub(super) fn collect_provider_models(
    provider: &Provider,
    exact: &mut Vec<String>,
    prefixes: &mut Vec<String>,
    selectable: &mut Vec<String>,
) {
    push_nonempty(exact, &provider.default_model);
    if let Some(model) = provider.subagent_model.as_deref() {
        push_nonempty(exact, model);
    }
    extend_nonempty(exact, &provider.selectable_models);
    extend_nonempty(prefixes, &provider.model_prefixes);
    if provider.enabled {
        extend_nonempty(selectable, &provider.selectable_models);
    }
}

pub(super) fn collect_route_models(
    route: &BackendRoute,
    exact: &mut Vec<String>,
    prefixes: &mut Vec<String>,
) {
    push_nonempty(exact, &route.model);
    extend_nonempty(prefixes, &route.model_prefixes);
}

fn push_nonempty(values: &mut Vec<String>, value: &str) {
    if !value.is_empty() {
        values.push(value.to_owned());
    }
}

fn extend_nonempty(values: &mut Vec<String>, candidates: &[String]) {
    values.extend(candidates.iter().filter(|value| !value.is_empty()).cloned());
}
