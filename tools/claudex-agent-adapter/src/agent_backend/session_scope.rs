use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, Mutex},
};

use super::{AgentBackend, BackendRoute, RoutedBackends};

/// Claude Code sessions without an id share this bucket (tests / anonymous HTTP).
pub(crate) const ANONYMOUS_SESSION_SCOPE: &str = "_anonymous";

/// One [`RoutedBackends`] pool per Claude Code session so Codex / ACP / Copilot
/// processes are not shared across unrelated Claude sessions.
pub struct SessionScopedBackends {
    templates: Vec<BackendRoute>,
    /// Metadata-only route table (never `get()` / start providers).
    catalog: RoutedBackends,
    scopes: Mutex<HashMap<String, Arc<AgentBackend>>>,
}

impl SessionScopedBackends {
    pub(super) fn new(routes: &[BackendRoute]) -> Self {
        Self {
            templates: routes.to_vec(),
            catalog: RoutedBackends::lazy(routes),
            scopes: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn scope_key(claude_session_id: Option<&str>) -> &str {
        match claude_session_id {
            Some(id) if !id.is_empty() => id,
            _ => ANONYMOUS_SESSION_SCOPE,
        }
    }

    pub(crate) fn scope(&self, claude_session_id: Option<&str>) -> Arc<AgentBackend> {
        let key = Self::scope_key(claude_session_id).to_owned();
        let mut scopes = self.scopes.lock().expect("session scopes poisoned");
        Arc::clone(scopes.entry(key).or_insert_with(|| {
            Arc::new(AgentBackend::Routed(RoutedBackends::lazy(&self.templates)))
        }))
    }

    pub(crate) fn catalog(&self) -> &RoutedBackends {
        &self.catalog
    }

    pub(crate) fn started_models(&self) -> Vec<String> {
        let scopes = self.scopes.lock().expect("session scopes poisoned");
        let mut models = BTreeSet::new();
        for backend in scopes.values() {
            for model in backend.started_models() {
                models.insert(model);
            }
        }
        models.into_iter().collect()
    }

    pub(crate) fn model_is_alive(&self, model: &str) -> bool {
        let scopes = self.scopes.lock().expect("session scopes poisoned");
        if scopes.is_empty() {
            return true;
        }
        scopes.values().all(|backend| backend.model_is_alive(model))
    }

    pub(crate) async fn release_scope(&self, claude_session_id: Option<&str>) {
        let key = Self::scope_key(claude_session_id).to_owned();
        let backend = self
            .scopes
            .lock()
            .expect("session scopes poisoned")
            .remove(&key);
        if let Some(backend) = backend {
            shutdown_scoped_pool(&backend).await;
        }
    }

    pub(crate) async fn shutdown_all(&self) {
        let backends = {
            let mut scopes = self.scopes.lock().expect("session scopes poisoned");
            scopes.drain().map(|(_, backend)| backend).collect::<Vec<_>>()
        };
        for backend in backends {
            shutdown_scoped_pool(&backend).await;
        }
    }

    #[cfg(test)]
    pub(crate) fn scope_count(&self) -> usize {
        self.scopes.lock().expect("session scopes poisoned").len()
    }
}

async fn shutdown_scoped_pool(backend: &AgentBackend) {
    match backend {
        AgentBackend::Routed(routes) => Box::pin(routes.shutdown()).await,
        // Scopes always store Routed pools; other variants are defensive.
        other => Box::pin(other.shutdown_leaf()).await,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::agent_backend::{BackendKind, BackendRoute, AgentBackend};
    use std::sync::Arc;

    #[test]
    fn scope_key_uses_anonymous_for_missing_ids() {
        assert_eq!(SessionScopedBackends::scope_key(None), ANONYMOUS_SESSION_SCOPE);
        assert_eq!(
            SessionScopedBackends::scope_key(Some("")),
            ANONYMOUS_SESSION_SCOPE
        );
        assert_eq!(SessionScopedBackends::scope_key(Some("sess-a")), "sess-a");
    }

    #[test]
    fn distinct_claude_sessions_get_independent_routed_pools() {
        let scopes = SessionScopedBackends::new(&[BackendRoute::new(
            "main",
            BackendKind::CodexAppServer,
        )]);
        let a = scopes.scope(Some("session-a"));
        let b = scopes.scope(Some("session-b"));
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(scopes.scope_count(), 2);
        assert!(Arc::ptr_eq(&a, &scopes.scope(Some("session-a"))));
    }

    #[tokio::test]
    async fn release_scope_drops_the_pool() {
        let scopes = SessionScopedBackends::new(&[BackendRoute::new(
            "main",
            BackendKind::CodexAppServer,
        )]);
        let _ = scopes.scope(Some("session-a"));
        assert_eq!(scopes.scope_count(), 1);
        scopes.release_scope(Some("session-a")).await;
        assert_eq!(scopes.scope_count(), 0);
    }

    #[test]
    fn empty_scopes_report_models_alive_and_catalog_metadata() {
        let scopes = SessionScopedBackends::new(&[BackendRoute::new(
            "main",
            BackendKind::CodexAppServer,
        )]);
        assert!(scopes.model_is_alive("main"));
        assert!(scopes.started_models().is_empty());
        assert!(scopes.catalog().supports("main"));
    }

    #[tokio::test]
    async fn shutdown_all_clears_every_scope() {
        let scopes = SessionScopedBackends::new(&[BackendRoute::new(
            "main",
            BackendKind::CodexAppServer,
        )]);
        let _ = scopes.scope(Some("a"));
        let _ = scopes.scope(Some("b"));
        assert_eq!(scopes.scope_count(), 2);
        scopes.shutdown_all().await;
        assert_eq!(scopes.scope_count(), 0);
    }

    #[test]
    fn scope_or_self_clones_non_scoped_backends() {
        let leaf = AgentBackend::spawn_routes(&[]);
        let scoped = leaf.scope_or_self(Some("sess"));
        assert!(!Arc::ptr_eq(&leaf, &scoped));
        let codex = AgentBackend::routed(Vec::new());
        assert!(Arc::ptr_eq(&codex, &codex.scope_or_self(Some("sess"))));
    }
}
