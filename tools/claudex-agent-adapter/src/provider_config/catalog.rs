use crate::agent_backend::BackendRoute;

use super::{
    ModelCatalog, Provider, WorkerRoute,
    identities::{collect_provider_models, collect_route_models},
};

impl ModelCatalog {
    pub(super) fn from_providers<'a>(providers: impl IntoIterator<Item = &'a Provider>) -> Self {
        let mut exact = Vec::new();
        let mut prefixes = Vec::new();
        let mut selectable = Vec::new();
        for provider in providers {
            collect_provider_models(provider, &mut exact, &mut prefixes, &mut selectable);
        }
        exact.sort();
        exact.dedup();
        prefixes.sort();
        prefixes.dedup();
        selectable.sort();
        selectable.dedup();
        Self {
            exact,
            prefixes,
            selectable,
            workers: Vec::new(),
            search_workers: Vec::new(),
            auxiliary_workers: Vec::new(),
        }
    }
    pub fn from_routes(routes: &[BackendRoute]) -> Self {
        let mut exact = Vec::new();
        let mut prefixes = Vec::new();
        for route in routes {
            collect_route_models(route, &mut exact, &mut prefixes);
        }
        exact.sort();
        exact.dedup();
        prefixes.sort();
        prefixes.dedup();
        Self {
            exact,
            prefixes,
            selectable: Vec::new(),
            workers: Vec::new(),
            search_workers: Vec::new(),
            auxiliary_workers: Vec::new(),
        }
    }
    pub fn selectable_models(&self) -> &[String] {
        &self.selectable
    }

    pub fn set_selectable_models(&mut self, models: Vec<String>) {
        let mut models: Vec<String> = models
            .into_iter()
            .filter(|model| !model.is_empty())
            .collect();
        models.sort();
        models.dedup();
        self.selectable = models;
    }
    pub fn matches(&self, model: &str) -> bool {
        self.exact.iter().any(|exact| exact == model)
            || self
                .prefixes
                .iter()
                .any(|prefix| model.starts_with(prefix.as_str()))
    }

    pub fn worker_fields(&self, agent: &str) -> Option<(&str, &str)> {
        self.workers
            .iter()
            .chain(self.auxiliary_workers.iter())
            .find(|worker| worker.agent == agent)
            .map(|worker| (worker.model.as_str(), worker.effort.as_str()))
    }
}

#[path = "catalog_routes.rs"]
mod routes;
