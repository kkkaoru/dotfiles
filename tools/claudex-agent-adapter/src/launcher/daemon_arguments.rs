use std::ffi::OsString;

use crate::{agent_backend::BackendRoute, provider_config::ModelCatalog};

use super::AdapterOptions;

pub(super) fn daemon_arguments(options: &AdapterOptions) -> Vec<OsString> {
    let mut arguments = vec!["serve".into()];
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
    arguments.extend([
        "--listen".into(),
        options.listen.to_string().into(),
        "--subscription-max-processes".into(),
        options.subscription_max_processes.to_string().into(),
        "--subscription-timeout-minutes".into(),
        options.subscription_timeout_minutes.to_string().into(),
    ]);
    arguments
}

pub(super) fn route_descriptions(routes: &[BackendRoute]) -> Vec<String> {
    routes.iter().map(BackendRoute::description).collect()
}

pub(super) fn worker_route_descriptions(catalog: &ModelCatalog) -> Vec<String> {
    catalog.worker_routes().iter().map(worker_json).collect()
}

pub(super) fn search_worker_route_descriptions(catalog: &ModelCatalog) -> Vec<String> {
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
