use crate::agent_backend::BackendRoute;

use super::Provider;

impl Provider {
    pub(super) fn required_fields(&self) -> [&str; 4] {
        [&self.id, &self.agent, &self.default_model, &self.effort]
    }

    pub(super) fn into_route(self) -> BackendRoute {
        let _ = self.usage_provider;
        let _ = self.usage_weekly_window_id;
        let _ = self.request_budget.map(|budget| {
            (
                budget.estimated_requests,
                budget.window_minutes,
                budget.usage_window,
            )
        });
        BackendRoute {
            model: self.default_model,
            backend: self.backend,
            effort: Some(self.effort),
            model_provider: self.model_provider,
            model_catalog_json: self.model_catalog_json,
            pi_provider: self.pi_provider,
            pi_model: self.pi_model,
            max_context_tokens: self.max_context_tokens,
            max_concurrency: self.max_concurrency,
            model_prefixes: self.model_prefixes,
            acp: self.acp,
            web_search_mode: self.web_search_mode,
        }
    }
}
