use std::ffi::OsString;

use crate::{agent_backend::BackendRoute, provider_config::ModelCatalog};

use super::AdapterOptions;

pub(super) fn daemon_arguments(options: &AdapterOptions) -> Vec<OsString> {
    let mut arguments = vec!["serve".into()];
    arguments.extend(option_arguments(options));
    arguments
}

pub(super) fn hot_swap_wait_arguments(options: &AdapterOptions) -> Vec<OsString> {
    let mut arguments = vec!["hot-swap".into(), "--wait-idle".into()];
    arguments.extend(option_arguments(options));
    arguments
}

fn option_arguments(options: &AdapterOptions) -> Vec<OsString> {
    let mut arguments = Vec::new();
    if !options.model.is_empty() {
        arguments.push("--model".into());
        arguments.push(options.model.clone().into());
    }
    for route in &options.routes {
        arguments.push("--backend-route-json".into());
        arguments.push(route_json(route).into());
    }
    for worker in options.model_catalog.worker_routes() {
        arguments.push("--worker-route-json".into());
        arguments.push(worker_json(worker).into());
    }
    for worker in options.model_catalog.search_worker_routes() {
        arguments.push("--search-worker-route-json".into());
        arguments.push(worker_json(worker).into());
    }
    for model in options.model_catalog.selectable_models() {
        arguments.push("--selectable-model".into());
        arguments.push(model.clone().into());
    }
    arguments.extend([
        "--listen".into(),
        options.listen.to_string().into(),
        "--subscription-max-processes".into(),
        options.subscription_max_processes.to_string().into(),
        "--subscription-timeout-minutes".into(),
        options.subscription_timeout_minutes.to_string().into(),
    ]);
    if let Some(seconds) = options.subagent_hard_timeout_seconds {
        arguments.push("--subagent-hard-timeout-seconds".into());
        arguments.push(seconds.get().to_string().into());
    }
    arguments
}

pub(crate) fn route_descriptions(routes: &[BackendRoute]) -> Vec<String> {
    routes.iter().map(BackendRoute::description).collect()
}

pub(crate) fn worker_route_descriptions(catalog: &ModelCatalog) -> Vec<String> {
    catalog.worker_routes().iter().map(worker_json).collect()
}

pub(crate) fn search_worker_route_descriptions(catalog: &ModelCatalog) -> Vec<String> {
    catalog
        .search_worker_routes()
        .iter()
        .map(worker_json)
        .collect()
}

fn route_json(route: &BackendRoute) -> String {
    serde_json::to_string(route).expect("backend route must serialize")
}

fn worker_json(worker: &crate::provider_config::WorkerRoute) -> String {
    serde_json::to_string(worker).expect("worker route must serialize")
}
