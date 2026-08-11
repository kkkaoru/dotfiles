use super::{AgentBackend, BackendKind, RoutedBackends};

impl RoutedBackends {
    pub(crate) fn max_concurrency_for_model(&self, model: &str) -> Option<usize> {
        if let Some(route) = self.find(model) {
            return route.template.max_concurrency;
        }
        self.prefix_template(model)
            .and_then(|template| template.max_concurrency)
    }

    pub(crate) fn configured_concurrency_limits(&self) -> Vec<(String, usize)> {
        self.configured
            .iter()
            .filter_map(|route| {
                route
                    .template
                    .max_concurrency
                    .map(|limit| (route.model.clone(), limit))
            })
            .collect()
    }

    pub(in crate::agent_backend) fn backend_kind_for_model(
        &self,
        model: &str,
    ) -> Option<BackendKind> {
        self.find(model)
            .map(|route| route.kind)
            .or_else(|| self.prefix_template(model).map(|template| template.backend))
    }

    pub(in crate::agent_backend) fn model_provider_for_model(&self, model: &str) -> Option<String> {
        self.find(model)
            .and_then(|route| route.template.model_provider.clone())
            .or_else(|| {
                self.prefix_template(model)
                    .and_then(|template| template.model_provider.clone())
            })
    }

    pub(in crate::agent_backend) fn first_ready(
        &self,
        kind: BackendKind,
    ) -> Option<std::sync::Arc<AgentBackend>> {
        self.configured
            .iter()
            .find(|route| route.kind == kind && route.ready_backend().is_some())
            .and_then(|route| route.ready_backend())
            .or_else(|| {
                self.dynamic
                    .lock()
                    .expect("dynamic routes poisoned")
                    .iter()
                    .find(|route| route.kind == kind && route.ready_backend().is_some())
                    .and_then(|route| route.ready_backend())
            })
    }
}
