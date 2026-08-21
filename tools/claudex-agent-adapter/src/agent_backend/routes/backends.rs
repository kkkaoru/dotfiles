use std::sync::{Arc, Mutex, atomic::AtomicBool};

use super::{AgentBackend, BackendKind, BackendRoute, BackendStartup, RoutedBackend};

pub struct RoutedBackends {
    pub(super) configured: Vec<Arc<RoutedBackend>>,
    pub(super) dynamic: Mutex<Vec<Arc<RoutedBackend>>>,
    pub(super) codex_startup: Arc<BackendStartup>,
    pub(super) closed: AtomicBool,
}

pub(super) fn startup_for_route(
    route: &BackendRoute,
    codex_startup: &Arc<BackendStartup>,
) -> Arc<BackendStartup> {
    if route.backend == BackendKind::CodexAppServer {
        return Arc::clone(codex_startup);
    }
    Arc::new(BackendStartup::default())
}

impl RoutedBackends {
    pub(in crate::agent_backend) fn lazy(routes: &[BackendRoute]) -> Self {
        let codex_startup = Arc::new(BackendStartup::default());
        Self {
            configured: routes
                .iter()
                .map(|route| {
                    Arc::new(RoutedBackend::lazy(
                        route.clone(),
                        startup_for_route(route, &codex_startup),
                    ))
                })
                .collect(),
            dynamic: Mutex::new(Vec::new()),
            codex_startup,
            closed: AtomicBool::new(false),
        }
    }

    pub(in crate::agent_backend) fn ready(routes: Vec<(String, Arc<AgentBackend>)>) -> Self {
        let configured = routes
            .into_iter()
            .map(|(model, backend)| Arc::new(RoutedBackend::ready(model, backend)))
            .collect::<Vec<_>>();
        let codex_startup = configured
            .iter()
            .find(|route| route.kind == BackendKind::CodexAppServer)
            .map(|route| Arc::clone(&route.startup))
            .unwrap_or_else(|| Arc::new(BackendStartup::default()));
        Self {
            configured,
            dynamic: Mutex::new(Vec::new()),
            codex_startup,
            closed: AtomicBool::new(false),
        }
    }

    pub(in crate::agent_backend) async fn search_backend(
        &self,
        model: &str,
    ) -> anyhow::Result<Arc<AgentBackend>> {
        let (_, route) = self.resolve(model)?;
        if route.kind != BackendKind::CodexAppServer {
            anyhow::bail!(
                "Pi gateway web search must use the provider-native route for model `{model}`"
            );
        }
        route.get().await
    }
}
