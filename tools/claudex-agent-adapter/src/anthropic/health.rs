use std::collections::BTreeMap;

use super::{Bridge, MAX_SESSIONS, model_concurrency::ModelConcurrencyStatus};

impl Bridge {
    pub(super) fn model_catalog(&self) -> &crate::provider_config::ModelCatalog {
        &self.model_catalog
    }

    pub fn is_alive(&self) -> bool {
        self.app.is_alive()
    }

    pub fn subscription_max_processes(&self) -> usize {
        self.subscription_max_processes
    }

    pub const fn session_capacity(&self) -> usize {
        MAX_SESSIONS
    }

    pub fn used_session_slots(&self) -> usize {
        MAX_SESSIONS - self.session_slots.available_permits()
    }

    pub fn subscription_timeout_minutes(&self) -> u64 {
        self.subscription_timeout.as_secs() / 60
    }

    pub fn backend_routes(&self) -> Vec<String> {
        self.app.route_descriptions()
    }

    pub fn worker_routes(&self) -> Vec<String> {
        self.model_catalog
            .worker_routes()
            .iter()
            .map(|worker| serde_json::to_string(worker).expect("worker route must serialize"))
            .collect()
    }

    pub fn search_worker_routes(&self) -> Vec<String> {
        self.model_catalog
            .search_worker_routes()
            .iter()
            .map(|worker| serde_json::to_string(worker).expect("worker route must serialize"))
            .collect()
    }

    pub(crate) async fn run_web_search(
        &self,
        query: &str,
    ) -> anyhow::Result<crate::web_search::SearchResponse> {
        crate::web_search::run(&self.app, self.model_catalog.search_worker_routes(), query).await
    }

    pub fn routed_models(&self) -> Vec<String> {
        let models = self.app.models();
        if models.is_empty() {
            vec![self.model.clone()]
        } else {
            models
        }
    }

    pub fn started_models(&self) -> Vec<String> {
        self.app.started_models()
    }

    pub(crate) fn model_concurrency(&self) -> BTreeMap<String, ModelConcurrencyStatus> {
        self.model_concurrency.snapshot()
    }
}
