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
        use std::collections::hash_map::Entry;
        match scopes.entry(key.clone()) {
            Entry::Occupied(entry) => {
                let backend = Arc::clone(entry.get());
                tracing::debug!(
                    target: "claudex.provider",
                    log_event = "provider_session_scope_reuse",
                    claude_session_id = %key,
                    provider_session_scope_count = scopes.len(),
                    "reusing Claude-session provider pool"
                );
                backend
            }
            Entry::Vacant(entry) => {
                let backend = Arc::new(AgentBackend::Routed(RoutedBackends::lazy(&self.templates)));
                entry.insert(Arc::clone(&backend));
                tracing::info!(
                    target: "claudex.provider",
                    log_event = "provider_session_scope_create",
                    claude_session_id = %key,
                    provider_session_scope_count = scopes.len(),
                    "created Claude-session provider pool"
                );
                backend
            }
        }
    }

    pub(crate) fn catalog(&self) -> &RoutedBackends {
        &self.catalog
    }

    pub(crate) fn scope_count(&self) -> usize {
        self.scopes.lock().expect("session scopes poisoned").len()
    }

    /// Snapshot of active Claude-session provider pools for /health and TUI.
    pub(crate) fn scope_snapshots(&self) -> Vec<ProviderSessionScopeSnapshot> {
        let scopes = self.scopes.lock().expect("session scopes poisoned");
        let mut snapshots = scopes
            .iter()
            .map(|(id, backend)| ProviderSessionScopeSnapshot {
                claude_session_id: id.clone(),
                started_models: backend.started_models(),
            })
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.claude_session_id.cmp(&right.claude_session_id));
        snapshots
    }

    /// Prefer the sole named Claude-session pool when an unguarded top-level
    /// call forgets `scope_or_self`. Multi-TUI still requires an explicit scope.
    pub(crate) fn unguarded_scope(&self) -> Arc<AgentBackend> {
        let scopes = self.scopes.lock().expect("session scopes poisoned");
        let mut named = scopes
            .iter()
            .filter(|(key, _)| key.as_str() != ANONYMOUS_SESSION_SCOPE);
        if let (Some((_, backend)), None) = (named.next(), named.next()) {
            return Arc::clone(backend);
        }
        drop(scopes);
        self.scope(None)
    }

    /// Pool that already started `model`. `None` if none or more than one match.
    pub(crate) fn unique_started_pool_for_model(&self, model: &str) -> Option<Arc<AgentBackend>> {
        let scopes = self.scopes.lock().expect("session scopes poisoned");
        let mut matches = scopes.values().filter(|backend| {
            backend
                .started_models()
                .iter()
                .any(|started| started == model)
        });
        let first = matches.next().cloned();
        if matches.next().is_some() {
            return None;
        }
        first
    }

    #[cfg(test)]
    pub(crate) fn insert_scope_for_test(&self, id: &str, backend: Arc<AgentBackend>) {
        self.scopes
            .lock()
            .expect("session scopes poisoned")
            .insert(id.to_owned(), backend);
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
            let remaining = self.scope_count();
            tracing::info!(
                target: "claudex.provider",
                log_event = "provider_session_scope_release",
                claude_session_id = %key,
                provider_session_scope_count = remaining,
                "shutting down Claude-session provider pool"
            );
            shutdown_scoped_pool(&backend).await;
        }
    }

    pub(crate) async fn shutdown_all(&self) {
        let backends = {
            let mut scopes = self.scopes.lock().expect("session scopes poisoned");
            let drained = scopes.drain().collect::<Vec<_>>();
            if !drained.is_empty() {
                tracing::info!(
                    target: "claudex.provider",
                    log_event = "provider_session_scope_shutdown_all",
                    provider_session_scope_count = drained.len(),
                    "shutting down every Claude-session provider pool"
                );
            }
            drained
                .into_iter()
                .map(|(_, backend)| backend)
                .collect::<Vec<_>>()
        };
        for backend in backends {
            shutdown_scoped_pool(&backend).await;
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProviderSessionScopeSnapshot {
    pub(crate) claude_session_id: String,
    pub(crate) started_models: Vec<String>,
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
    include!("session_scope_tests.rs");
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "session_scope_routing_tests.rs"]
mod routing_tests;
