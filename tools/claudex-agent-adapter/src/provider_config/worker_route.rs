use super::WorkerRoute;

impl WorkerRoute {
    pub fn new(
        agent: impl Into<String>,
        model: impl Into<String>,
        effort: impl Into<String>,
    ) -> Self {
        Self {
            agent: agent.into(),
            model: model.into(),
            effort: effort.into(),
            usage_provider: None,
        }
    }

    pub(crate) fn with_usage_provider(mut self, usage_provider: Option<String>) -> Self {
        self.usage_provider = usage_provider;
        self
    }

    pub(crate) fn usage_provider(&self) -> Option<&str> {
        self.usage_provider.as_deref()
    }
}

impl super::ModelCatalog {
    pub(crate) fn usage_provider_for_model(&self, model: &str) -> Option<&str> {
        self.worker_routes()
            .iter()
            .chain(self.search_worker_routes())
            .find(|worker| worker.model == model)
            .and_then(WorkerRoute::usage_provider)
    }
}
