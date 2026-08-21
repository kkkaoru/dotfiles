use std::sync::{Arc, Mutex, atomic::AtomicBool};

use super::{AgentBackend, BackendKind, BackendRoute, BackendStartup, RoutedBackend};

pub struct RoutedBackends {
    pub(super) configured: Vec<Arc<RoutedBackend>>,
    pub(super) dynamic: Mutex<Vec<Arc<RoutedBackend>>>,
    pub(super) search_routes: Mutex<Vec<Arc<RoutedBackend>>>,
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
            search_routes: Mutex::new(Vec::new()),
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
            search_routes: Mutex::new(Vec::new()),
            codex_startup,
            closed: AtomicBool::new(false),
        }
    }

    pub(in crate::agent_backend) async fn search_backend(
        &self,
        model: &str,
    ) -> anyhow::Result<Arc<AgentBackend>> {
        self.search_route(model).get().await
    }

    pub(in crate::agent_backend) fn search_route(&self, model: &str) -> Arc<RoutedBackend> {
        let mut search_routes = self.search_routes.lock().expect("search routes poisoned");
        if let Some(existing) = search_routes.iter().find(|route| route.model == model) {
            return Arc::clone(existing);
        }
        let mut template = BackendRoute::new(model, BackendKind::CodexAppServer);
        template.web_search_mode = crate::agent_backend::WebSearchMode::CodexNative;
        let route = Arc::new(RoutedBackend::lazy(
            template,
            Arc::clone(&self.codex_startup),
        ));
        search_routes.push(Arc::clone(&route));
        route
    }
}
