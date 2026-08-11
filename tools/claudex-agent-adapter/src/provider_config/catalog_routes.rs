use anyhow::Result;

use super::{ModelCatalog, Provider, WorkerRoute};
use super::super::validation::validate_worker_routes;

impl ModelCatalog {
    pub fn worker_effort_for_model(&self, model: &str) -> Option<&str> {
        self.workers
            .iter()
            .chain(self.search_workers.iter())
            .chain(self.auxiliary_workers.iter())
            .find(|worker| worker.model == model)
            .map(|worker| worker.effort.as_str())
    }

    pub fn worker_agent_for_model(&self, model: &str) -> Option<&str> {
        self.workers
            .iter()
            .chain(self.auxiliary_workers.iter())
            .find(|worker| worker.model == model)
            .map(|worker| worker.agent.as_str())
    }

    pub fn worker_routes(&self) -> &[WorkerRoute] {
        &self.workers
    }

    /// Returns the deterministic Claudex-managed route for a generic Agent.
    /// Generic Agent types must not inherit the outer Claude session model.
    pub fn default_worker_fields(&self) -> Option<(&str, &str)> {
        self.workers
            .first()
            .map(|route| (route.model.as_str(), route.effort.as_str()))
            .or_else(|| {
                self.auxiliary_workers
                    .first()
                    .map(|route| (route.model.as_str(), route.effort.as_str()))
            })
    }

    /// Configured Claude subscription fallback from `providers.json` (`fallback`).
    pub fn configured_fallback(&self) -> Option<(&str, &str)> {
        self.auxiliary_workers
            .first()
            .map(|route| (route.model.as_str(), route.effort.as_str()))
    }

    pub fn search_worker_routes(&self) -> &[WorkerRoute] {
        &self.search_workers
    }

    pub fn with_search_worker_routes(mut self, workers: Vec<WorkerRoute>) -> Result<Self> {
        self.set_search_worker_routes(workers)?;
        Ok(self)
    }

    pub fn set_search_worker_routes(&mut self, workers: Vec<WorkerRoute>) -> Result<()> {
        validate_worker_routes(&workers)?;
        self.search_workers = workers;
        Ok(())
    }

    pub fn set_worker_routes(&mut self, workers: Vec<WorkerRoute>) -> Result<()> {
        validate_worker_routes(&workers)?;
        self.workers = workers;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn push_worker_unchecked_for_tests(&mut self, worker: WorkerRoute) {
        self.workers.push(worker);
    }

    pub(crate) fn set_auxiliary_worker_routes(&mut self, workers: Vec<WorkerRoute>) -> Result<()> {
        validate_worker_routes(&workers)?;
        self.auxiliary_workers = workers;
        Ok(())
    }

    pub(in crate::provider_config) fn add_workers(
        &mut self,
        providers: &[Provider],
        native_workers: &[WorkerRoute],
    ) -> Result<()> {
        let mut workers = providers
            .iter()
            .map(|provider| {
                WorkerRoute::new(
                    provider.agent.clone(),
                    provider
                        .subagent_model
                        .as_ref()
                        .unwrap_or(&provider.default_model)
                        .clone(),
                    provider.effort.clone(),
                )
                .with_usage_provider(provider.usage_provider.clone())
            })
            .collect::<Vec<_>>();
        workers.extend_from_slice(native_workers);
        self.set_worker_routes(workers)
    }
}
