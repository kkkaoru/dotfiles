use std::collections::{BTreeMap, BTreeSet};

use super::{Bridge, MAX_SESSIONS, Session, model_concurrency::ModelConcurrencyStatus};

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

    pub(crate) async fn run_web_search_for_session(
        &self,
        query: &str,
        session_id: Option<&str>,
    ) -> anyhow::Result<crate::web_search::SearchResponse> {
        if let Some(response) = self.run_session_pi_search(query, session_id).await? {
            return Ok(response);
        }
        crate::web_search::run(&self.app, self.model_catalog.search_worker_routes(), query).await
    }

    async fn run_session_pi_search(
        &self,
        query: &str,
        session_id: Option<&str>,
    ) -> anyhow::Result<Option<crate::web_search::SearchResponse>> {
        let Some(model) = self.session_model(session_id).await else {
            return Ok(None);
        };
        if self.app.web_search_mode(&model) != crate::web_search::WebSearchMode::DelegatePi {
            return Ok(None);
        }
        let Some((provider, pi_model)) = self.app.pi_identity(&model) else {
            return Ok(None);
        };
        crate::web_search::run_pi(&self.app, &provider, &pi_model, query).await
    }

    async fn session_model(&self, session_id: Option<&str>) -> Option<String> {
        let session_id = session_id.filter(|id| !id.is_empty())?;
        let sessions = self.sessions.lock().await;
        sessions.iter().find_map(|session| {
            session
                .claude_session_id
                .as_deref()
                .filter(|id| *id == session_id)
                .map(|_| session.model.clone())
        })
    }

    pub fn routed_models(&self) -> Vec<String> {
        let mut models = self.app.models();
        models.extend(self.model_catalog.selectable_models().iter().cloned());
        for worker in self
            .model_catalog
            .worker_routes()
            .iter()
            .chain(self.model_catalog.search_worker_routes().iter())
            .filter(|worker| !worker.model.is_empty())
        {
            models.push(worker.model.clone());
        }
        if let Some((model, _)) = self.model_catalog.configured_fallback() {
            models.push(model.to_owned());
        }
        models.sort();
        models.dedup();
        if models.is_empty() {
            vec![self.model.clone()]
        } else {
            models
        }
    }

    pub fn started_models(&self) -> Vec<String> {
        self.app.started_models()
    }

    pub(crate) fn provider_session_scope_count(&self) -> usize {
        self.app.provider_session_scope_count()
    }

    pub(crate) fn provider_session_scopes(
        &self,
    ) -> Vec<crate::agent_backend::ProviderSessionScopeSnapshot> {
        self.app.provider_session_scopes()
    }

    pub(crate) fn model_concurrency(&self) -> BTreeMap<String, ModelConcurrencyStatus> {
        self.model_concurrency.snapshot()
    }

    pub(crate) fn active_subagent_models(&self) -> BTreeMap<String, usize> {
        self.active_subagent_models.snapshot()
    }

    pub(crate) fn active_subagent_agent_ids(&self) -> Vec<String> {
        self.active_subagent_models.active_agent_ids()
    }

    pub(crate) fn recent_subagent_agent_ids(&self) -> BTreeMap<String, u64> {
        self.active_subagent_models
            .recent_agent_ages(std::time::Instant::now())
    }

    pub async fn active_claude_session_ids(&self) -> Vec<String> {
        let mut ids = BTreeSet::new();
        collect_session_ids(&self.sessions.lock().await, &mut ids);
        collect_session_ids(&self.detached_sessions.lock().await, &mut ids);
        ids.into_iter().collect()
    }

    pub async fn busy_claude_session_ids(&self) -> Vec<String> {
        let mut ids = BTreeSet::new();
        collect_busy_session_ids(&self.sessions.lock().await, &mut ids);
        collect_session_ids(&self.detached_sessions.lock().await, &mut ids);
        ids.into_iter().collect()
    }
}

fn collect_session_ids(sessions: &[std::sync::Arc<Session>], ids: &mut BTreeSet<String>) {
    for session in sessions {
        if let Some(id) = session.claude_session_id.as_deref()
            && !id.is_empty()
        {
            ids.insert(id.to_owned());
        }
    }
}

fn collect_busy_session_ids(sessions: &[std::sync::Arc<Session>], ids: &mut BTreeSet<String>) {
    for session in sessions {
        if !session_is_busy(session) {
            continue;
        }
        if let Some(id) = session.claude_session_id.as_deref()
            && !id.is_empty()
        {
            ids.insert(id.to_owned());
        }
    }
}

fn session_is_busy(session: &Session) -> bool {
    if session.gate.try_lock().is_err() {
        return true;
    }
    session
        .pending_tools
        .try_lock()
        .map(|pending| !pending.is_empty())
        .unwrap_or(true)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "health_tests.rs"]
mod health_tests;
