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
                drop(entry);
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
    use super::*;
    use crate::agent_backend::{AgentBackend, BackendKind, BackendRoute};
    use std::sync::Arc;

    #[test]
    fn scope_key_uses_anonymous_for_missing_ids() {
        assert_eq!(
            SessionScopedBackends::scope_key(None),
            ANONYMOUS_SESSION_SCOPE
        );
        assert_eq!(
            SessionScopedBackends::scope_key(Some("")),
            ANONYMOUS_SESSION_SCOPE
        );
        assert_eq!(SessionScopedBackends::scope_key(Some("sess-a")), "sess-a");
    }

    #[test]
    fn distinct_claude_sessions_get_independent_routed_pools() {
        let scopes =
            SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
        let a = scopes.scope(Some("session-a"));
        let b = scopes.scope(Some("session-b"));
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(scopes.scope_count(), 2);
        assert!(Arc::ptr_eq(&a, &scopes.scope(Some("session-a"))));
    }

    #[tokio::test]
    async fn release_scope_drops_the_pool() {
        let scopes =
            SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
        let _ = scopes.scope(Some("session-a"));
        assert_eq!(scopes.scope_count(), 1);
        scopes.release_scope(Some("session-a")).await;
        assert_eq!(scopes.scope_count(), 0);
    }

    #[test]
    fn empty_scopes_report_models_alive_and_catalog_metadata() {
        let scopes =
            SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
        assert!(scopes.model_is_alive("main"));
        assert!(scopes.started_models().is_empty());
        assert!(scopes.catalog().supports("main"));
    }

    #[tokio::test]
    async fn shutdown_all_clears_every_scope() {
        let scopes =
            SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
        let _ = scopes.scope(Some("a"));
        let _ = scopes.scope(Some("b"));
        assert_eq!(scopes.scope_count(), 2);
        scopes.shutdown_all().await;
        assert_eq!(scopes.scope_count(), 0);
    }

    #[test]
    fn scope_snapshots_sort_and_report_started_models() {
        let scopes =
            SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
        let _ = scopes.scope(Some("sess-b"));
        let _ = scopes.scope(Some("sess-a"));
        let snapshots = scopes.scope_snapshots();
        assert_eq!(
            snapshots
                .iter()
                .map(|snapshot| snapshot.claude_session_id.as_str())
                .collect::<Vec<_>>(),
            ["sess-a", "sess-b"]
        );
        assert!(
            snapshots
                .iter()
                .all(|snapshot| snapshot.started_models.is_empty())
        );
    }

    #[test]
    fn unique_started_pool_is_none_for_lazy_scopes() {
        let scopes = SessionScopedBackends::new(&[BackendRoute::new(
            "glm-5.2:cloud",
            BackendKind::CodexAppServer,
        )]);
        let _ = scopes.scope(Some("tui-session"));
        assert!(
            scopes
                .unique_started_pool_for_model("glm-5.2:cloud")
                .is_none()
        );
    }

    #[test]
    fn scope_or_self_clones_non_scoped_backends() {
        let leaf = AgentBackend::spawn_routes(&[]);
        let scoped = leaf.scope_or_self(Some("sess"));
        assert!(!Arc::ptr_eq(&leaf, &scoped));
        let codex = AgentBackend::routed(Vec::new());
        assert!(Arc::ptr_eq(&codex, &codex.scope_or_self(Some("sess"))));
    }

    #[test]
    fn scope_create_and_reuse_emit_structured_log_events() {
        use tracing::level_filters::LevelFilter;

        let buffer = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        struct BufferWriter(Arc<std::sync::Mutex<Vec<u8>>>);
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufferWriter {
            type Writer = BufferWriter;

            fn make_writer(&'a self) -> Self::Writer {
                BufferWriter(Arc::clone(&self.0))
            }
        }
        impl std::io::Write for BufferWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("log buffer").extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(LevelFilter::DEBUG)
            .with_writer(BufferWriter(Arc::clone(&buffer)))
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let scopes =
            SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
        let _ = scopes.scope(Some("log-sess"));
        let _ = scopes.scope(Some("log-sess"));
        let text = String::from_utf8(buffer.lock().expect("log buffer").clone()).unwrap();
        assert!(
            text.contains("provider_session_scope_create"),
            "missing create event in logs: {text}"
        );
        assert!(
            text.contains("provider_session_scope_reuse"),
            "missing reuse event in logs: {text}"
        );
        assert!(
            text.contains("log-sess"),
            "missing session id in logs: {text}"
        );
    }

    #[tokio::test]
    async fn scope_release_emits_structured_log_event() {
        use tracing::level_filters::LevelFilter;

        let buffer = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        struct BufferWriter(Arc<std::sync::Mutex<Vec<u8>>>);
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufferWriter {
            type Writer = BufferWriter;

            fn make_writer(&'a self) -> Self::Writer {
                BufferWriter(Arc::clone(&self.0))
            }
        }
        impl std::io::Write for BufferWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("log buffer").extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(LevelFilter::INFO)
            .with_writer(BufferWriter(Arc::clone(&buffer)))
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let scopes =
            SessionScopedBackends::new(&[BackendRoute::new("main", BackendKind::CodexAppServer)]);
        let _ = scopes.scope(Some("release-sess"));
        scopes.release_scope(Some("release-sess")).await;
        let text = String::from_utf8(buffer.lock().expect("log buffer").clone()).unwrap();
        assert!(
            text.contains("provider_session_scope_release"),
            "missing release event in logs: {text}"
        );
        assert!(
            text.contains("release-sess"),
            "missing session id in logs: {text}"
        );
    }
}
